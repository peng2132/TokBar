pub mod adapters;
pub mod aggregate;
pub mod cost;
pub mod db;
pub mod pricing;
pub mod types;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(target_os = "macos")]
use tauri::utils::config::WindowEffectsConfig;
#[cfg(target_os = "macos")]
use tauri::utils::{WindowEffect, WindowEffectState};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::cost::CostMode;
use crate::pricing::PricingMap;

/// macOS menu bar wants a monochrome template icon; Windows/Linux trays
/// render the icon as-is, so ship the colored app icon there.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

/// User settings persisted to settings.json in the app data dir.
#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    /// Menu bar text mode: "cost" | "tokens" | "off"
    tray_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tray_mode: "cost".to_string(),
        }
    }
}

struct AppState {
    conn: Mutex<rusqlite::Connection>,
    pricing: Mutex<PricingMap>,
    cache_dir: PathBuf,
    settings: Mutex<Settings>,
    /// Serializes whole scans (manual refresh vs. file watcher) while
    /// leaving `conn` free for readers during the slow parse phase.
    scan_lock: Mutex<()>,
}

fn settings_path(cache_dir: &PathBuf) -> PathBuf {
    cache_dir.join("settings.json")
}

fn load_settings(cache_dir: &PathBuf) -> Settings {
    std::fs::read_to_string(settings_path(cache_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(cache_dir: &PathBuf, settings: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(settings_path(cache_dir), json);
    }
}

fn format_tokens_short(n: i64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.1}K", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

fn parse_mode(mode: Option<String>) -> CostMode {
    CostMode::from_str(mode.as_deref().unwrap_or("auto"))
}

#[tauri::command]
fn refresh_data(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<db::ScanStats, String> {
    let stats = {
        let _scan_guard = state.scan_lock.lock().map_err(|e| e.to_string())?;
        let pricing = state.pricing.lock().map_err(|e| e.to_string())?;
        let progress_app = app.clone();
        db::scan_all(&state.conn, &pricing, move |done, total| {
            let _ = progress_app.emit("scan-progress", serde_json::json!({
                "done": done, "total": total
            }));
        })?
    };
    update_tray_title(&app, &state);
    Ok(stats)
}

/// Render today's usage next to the menu bar icon (macOS), according to
/// the configured tray display mode.
fn update_tray_title(app: &tauri::AppHandle, state: &tauri::State<AppState>) {
    let settings = state
        .settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    let Ok(conn) = state.conn.lock() else { return };
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (cost, tokens): (f64, i64) = conn
        .query_row(
            "SELECT COALESCE(SUM(COALESCE(cost_usd, calculated_cost)),0),
                    COALESCE(SUM(total_tokens),0)
             FROM entries WHERE date_local = ?1",
            [&today],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0.0, 0));
    drop(conn);

    if let Some(tray) = app.tray_by_id("main") {
        // "off" passes an empty string: set_title(None) does not clear an
        // already-set title on macOS.
        let title = match settings.tray_mode.as_str() {
            "off" => String::new(),
            "tokens" => format_tokens_short(tokens),
            _ => format!("${cost:.2}"),
        };
        let _ = tray.set_title(Some(title));
        let _ = tray.set_tooltip(Some(format!(
            "TokBar — today ${cost:.2} / {}",
            format_tokens_short(tokens)
        )));
    }
}

#[tauri::command]
fn set_tray_mode(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    mode: String,
) -> Result<(), String> {
    if !matches!(mode.as_str(), "cost" | "tokens" | "off") {
        return Err(format!("invalid tray mode: {mode}"));
    }
    if let Ok(mut s) = state.settings.lock() {
        s.tray_mode = mode;
        save_settings(&state.cache_dir, &s);
    }
    update_tray_title(&app, &state);
    Ok(())
}

#[tauri::command]
fn get_tray_mode(state: tauri::State<AppState>) -> String {
    state
        .settings
        .lock()
        .map(|s| s.tray_mode.clone())
        .unwrap_or_else(|_| "cost".to_string())
}

/// Watch all agent log directories and rescan automatically (debounced)
/// when anything changes, then notify every window.
fn spawn_usage_watcher(handle: tauri::AppHandle) {
    use notify::{RecursiveMode, Watcher};
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            },
        ) {
            Ok(w) => w,
            Err(_) => return,
        };
        let dirs: Vec<PathBuf> = adapters::ALL
            .iter()
            .flat_map(|a| (a.data_dirs)())
            .collect();
        for dir in &dirs {
            let _ = watcher.watch(dir, RecursiveMode::Recursive);
        }
        if dirs.is_empty() {
            return;
        }
        while rx.recv().is_ok() {
            // Debounce: wait until the directories have been quiet for 2s.
            while rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok()
            {}
            scan_and_broadcast(&handle);
        }
    });
}

fn scan_and_broadcast(handle: &tauri::AppHandle) {
    let state: tauri::State<AppState> = handle.state();
    {
        let Ok(_scan_guard) = state.scan_lock.lock() else { return };
        let Ok(pricing) = state.pricing.lock() else { return };
        let progress_app = handle.clone();
        let _ = db::scan_all(&state.conn, &pricing, move |done, total| {
            let _ = progress_app.emit("scan-progress", serde_json::json!({
                "done": done, "total": total
            }));
        });
    }
    update_tray_title(handle, &state);
    let _ = handle.emit("usage-updated", ());
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

/// Background auto-refresh of LiteLLM pricing: on launch (when the local
/// cache is older than 24h) and once a day thereafter. New rates apply to
/// newly scanned usage only — historical costs keep the rates in effect
/// when they were recorded.
fn spawn_pricing_auto_refresh(handle: tauri::AppHandle) {
    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
    std::thread::spawn(move || loop {
        let state: tauri::State<AppState> = handle.state();
        let cache_file = state.cache_dir.join("litellm-pricing.json");
        let stale = std::fs::metadata(&cache_file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map_or(true, |age| age > DAY);
        if stale && PricingMap::refresh_online(&state.cache_dir).is_ok() {
            if let Ok(mut pricing) = state.pricing.lock() {
                *pricing = PricingMap::load(Some(state.cache_dir.clone()));
            }
        }
        // Re-check every 6 hours; the mtime gate makes this refresh at
        // most once a day.
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
    });
}

#[tauri::command]
fn get_overview(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<aggregate::Overview, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::overview(&conn, since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command]
fn get_daily(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::DailyRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::daily(&conn, since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command]
fn get_hourly(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::DailyRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::hourly(&conn, since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command]
fn get_models(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::ModelRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::models(&conn, since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command]
fn get_sessions(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<aggregate::SessionRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::sessions(
        &conn,
        since_ms,
        until_ms,
        parse_mode(cost_mode),
        limit.unwrap_or(200),
    )
}

#[tauri::command]
fn get_projects(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<aggregate::ProjectRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::projects(
        &conn,
        since_ms,
        until_ms,
        parse_mode(cost_mode),
        limit.unwrap_or(20),
    )
}

#[tauri::command]
fn get_blocks(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::Block>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::blocks(&conn, since_ms, parse_mode(cost_mode), 5.0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceInfo {
    agent: String,
    dirs: Vec<String>,
    file_count: usize,
}

#[tauri::command]
fn get_sources() -> Vec<SourceInfo> {
    adapters::ALL
        .iter()
        .map(|a| SourceInfo {
            agent: a.agent.to_string(),
            dirs: (a.data_dirs)()
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            file_count: (a.collect_files)().len(),
        })
        .collect()
}

#[tauri::command]
fn get_session_models(
    state: tauri::State<AppState>,
    agent: String,
    session_id: String,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::ModelRow>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    aggregate::session_models(&conn, &agent, &session_id, parse_mode(cost_mode))
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    // Hidden frameless popover window, shown next to the tray icon on click.
    let quick = WebviewWindowBuilder::new(app, "quick", WebviewUrl::App("index.html".into()))
        .title("TokBar")
        .inner_size(360.0, 460.0)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false);
    // Transparent window + self-drawn rounded corners is a macOS look;
    // WebView2 transparency on Windows composites poorly (artifacts show
    // through), so the window stays opaque there. The HUD-window vibrancy
    // behind the webview gives the panel its frosted-glass material (the
    // radius matches the panel div's rounded-2xl).
    #[cfg(target_os = "macos")]
    let quick = quick.transparent(true).effects(WindowEffectsConfig {
        effects: vec![WindowEffect::HudWindow],
        state: Some(WindowEffectState::Active),
        radius: Some(16.0),
        color: None,
    });
    quick.build()?;

    let show = MenuItem::with_id(app, "show", "打开 TokBar / Open TokBar", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 / Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(TRAY_ICON)?)
        .icon_as_template(cfg!(target_os = "macos"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("quick") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        // macOS menu bar is at the top, so the panel opens
                        // below the icon; Windows/Linux trays sit at the
                        // bottom, so it opens above (TrayCenter).
                        let pos = if cfg!(target_os = "macos") {
                            Position::TrayBottomCenter
                        } else {
                            Position::TrayCenter
                        };
                        let _ = w.move_window(pos);
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            let conn = db::open(&data_dir.join("tokbar.db")).expect("failed to open database");
            let pricing = PricingMap::load(Some(data_dir.clone()));
            let settings = load_settings(&data_dir);
            app.manage(AppState {
                conn: Mutex::new(conn),
                pricing: Mutex::new(pricing),
                cache_dir: data_dir,
                settings: Mutex::new(settings),
                scan_lock: Mutex::new(()),
            });
            setup_tray(app)?;
            spawn_pricing_auto_refresh(app.handle().clone());
            spawn_usage_watcher(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Menu-bar app behavior: closing the main window hides it,
            // the app keeps running in the tray (quit via tray menu).
            tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The quick popover hides itself when it loses focus.
            tauri::WindowEvent::Focused(false) if window.label() == "quick" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            refresh_data,
            show_main_window,
            set_tray_mode,
            get_tray_mode,
            get_session_models,
            get_overview,
            get_daily,
            get_hourly,
            get_models,
            get_sessions,
            get_projects,
            get_blocks,
            get_sources
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // macOS: clicking the Dock icon after the red close button hid the
    // main window brings it back (fires only on macOS).
    app.run(|handle, event| {
        if let tauri::RunEvent::Reopen { .. } = event {
            show_main_window(handle.clone());
        }
    });
}
