pub mod adapters;
pub mod aggregate;
pub mod cost;
pub mod db;
#[cfg(target_os = "macos")]
pub mod notch;
pub mod pricing;
pub mod types;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(target_os = "macos")]
use tauri::utils::config::WindowEffectsConfig;
#[cfg(target_os = "macos")]
use tauri::utils::{WindowEffect, WindowEffectState};
use tauri_plugin_positioner::{Position, WindowExt};

use crate::cost::CostMode;
use crate::db::lock;
use crate::pricing::PricingMap;

/// macOS menu bar wants a monochrome template icon; Windows/Linux trays
/// render the icon as-is, so ship the colored app icon there.
#[cfg(target_os = "macos")]
const TRAY_ICON: &[u8] = include_bytes!("../icons/tray-icon.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: &[u8] = include_bytes!("../icons/32x32.png");

/// Online prices older than this are refreshed in the background.
const PRICING_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// User settings persisted to settings.json in the app data dir.
#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    /// Menu bar text when the notch bar is not in use: "cost" | "tokens" | "off"
    tray_mode: String,
    /// Notch companion bar (only takes effect on notched MacBooks).
    notch_enabled: bool,
    /// Cost mode shared by every window and the menu bar title.
    cost_mode: String,
    /// UI language ("en" | "zh"), for the tray tooltip.
    language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tray_mode: "cost".to_string(),
            notch_enabled: true,
            cost_mode: "auto".to_string(),
            language: "en".to_string(),
        }
    }
}

/// Geometry of the built-in display's notch, in logical px. All zeros on
/// machines (and platforms) without one.
#[derive(Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotchInfo {
    pub has_notch: bool,
    pub notch_width: f64,
    pub bar_height: f64,
    pub screen_width: f64,
}

#[derive(Default)]
struct PricingRuntime {
    refreshing: bool,
    last_error: Option<String>,
}

struct AppState {
    conn: Mutex<rusqlite::Connection>,
    /// Swapped wholesale on refresh; scans hold their own `Arc`, so
    /// queries never wait for a scan to finish.
    pricing: RwLock<Arc<PricingMap>>,
    pricing_rt: Mutex<PricingRuntime>,
    cache_dir: PathBuf,
    settings: Mutex<Settings>,
    /// Serializes whole scans (manual refresh, file watcher, pricing
    /// refresh) while leaving `conn` free for readers during the slow
    /// parse phase.
    scan_lock: Mutex<()>,
    /// Re-measured periodically on the main thread (displays come and go:
    /// clamshell mode, scaling changes).
    notch_info: Mutex<NotchInfo>,
}

impl AppState {
    fn pricing(&self) -> Arc<PricingMap> {
        self.pricing
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = lock(&self.settings);
        f(&mut s);
        save_settings(&self.cache_dir, &s);
    }

    fn notch_active(&self) -> bool {
        lock(&self.notch_info).has_notch && lock(&self.settings).notch_enabled
    }
}

fn settings_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("settings.json")
}

fn load_settings(cache_dir: &Path) -> Settings {
    std::fs::read_to_string(settings_path(cache_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(cache_dir: &Path, settings: &Settings) {
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

/// Run one incremental scan (re-pricing first if prices changed), then
/// refresh the menu bar and tell every window when data changed.
fn run_scan(app: &AppHandle) -> Result<db::ScanStats, String> {
    let state = app.state::<AppState>();
    let stats = {
        let _scan_guard = lock(&state.scan_lock);
        let pricing = state.pricing();
        let progress_app = app.clone();
        db::scan_all(&state.conn, &pricing, move |done, total| {
            let _ = progress_app.emit(
                "scan-progress",
                serde_json::json!({ "done": done, "total": total }),
            );
        })
    };
    // Always settle the progress indicator, also after a failed scan.
    let _ = app.emit("scan-progress", serde_json::json!({ "done": 0, "total": 0 }));
    let stats = stats?;
    update_tray_title(app);
    if stats.changed() {
        let _ = app.emit("usage-updated", ());
    }
    Ok(stats)
}

#[tauri::command]
async fn refresh_data(app: AppHandle) -> Result<db::ScanStats, String> {
    tauri::async_runtime::spawn_blocking(move || run_scan(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Render today's usage next to the menu bar icon (macOS), according to
/// the configured tray display mode and cost mode. While the notch bar is
/// showing it carries the number, so the menu bar keeps just the icon.
fn update_tray_title(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings();
    let mode = CostMode::from_str(&settings.cost_mode);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let (cost, tokens): (f64, i64) = lock(&state.conn)
        .query_row(
            &format!(
                "SELECT COALESCE(SUM({}),0), COALESCE(SUM(total_tokens),0)
                 FROM entries WHERE date_local = ?1",
                aggregate::cost_expr(mode)
            ),
            [&today],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((0.0, 0));

    if let Some(tray) = app.tray_by_id("main") {
        // "off" passes an empty string: set_title(None) does not clear an
        // already-set title on macOS.
        let title = if state.notch_active() {
            String::new()
        } else {
            match settings.tray_mode.as_str() {
                "off" => String::new(),
                "tokens" => format_tokens_short(tokens),
                _ => format!("${cost:.2}"),
            }
        };
        let _ = tray.set_title(Some(title));
        let tooltip = if settings.language == "zh" {
            format!("TokBar — 今日 ${cost:.2} / {} tokens", format_tokens_short(tokens))
        } else {
            format!("TokBar — today ${cost:.2} / {} tokens", format_tokens_short(tokens))
        };
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

#[tauri::command]
fn set_tray_mode(app: AppHandle, state: tauri::State<AppState>, mode: String) -> Result<(), String> {
    if !matches!(mode.as_str(), "cost" | "tokens" | "off") {
        return Err(format!("invalid tray mode: {mode}"));
    }
    state.update_settings(|s| s.tray_mode = mode);
    update_tray_title(&app);
    Ok(())
}

#[tauri::command]
fn get_tray_mode(state: tauri::State<AppState>) -> String {
    state.settings().tray_mode
}

#[tauri::command]
fn get_cost_mode(state: tauri::State<AppState>) -> String {
    state.settings().cost_mode
}

#[tauri::command]
fn set_cost_mode(app: AppHandle, state: tauri::State<AppState>, mode: String) -> Result<(), String> {
    if !matches!(mode.as_str(), "auto" | "calculate" | "display") {
        return Err(format!("invalid cost mode: {mode}"));
    }
    state.update_settings(|s| s.cost_mode = mode.clone());
    update_tray_title(&app);
    let _ = app.emit("cost-mode-changed", mode);
    Ok(())
}

#[tauri::command]
fn set_language(app: AppHandle, state: tauri::State<AppState>, lang: String) -> Result<(), String> {
    if !matches!(lang.as_str(), "en" | "zh") {
        return Err(format!("invalid language: {lang}"));
    }
    state.update_settings(|s| s.language = lang);
    update_tray_title(&app);
    Ok(())
}

#[tauri::command]
fn get_notch_info(state: tauri::State<AppState>) -> NotchInfo {
    *lock(&state.notch_info)
}

#[tauri::command]
fn get_notch_enabled(state: tauri::State<AppState>) -> bool {
    state.settings().notch_enabled
}

#[tauri::command]
fn set_notch_enabled(app: AppHandle, state: tauri::State<AppState>, enabled: bool) -> Result<(), String> {
    state.update_settings(|s| s.notch_enabled = enabled);
    sync_notch_window(&app);
    update_tray_title(&app);
    Ok(())
}

/// Create or close the notch window to match the current display and
/// setting (macOS only).
fn sync_notch_window(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let state = app.state::<AppState>();
        let info = *lock(&state.notch_info);
        if info.has_notch && state.settings().notch_enabled {
            notch::create_window(app, info);
        } else if let Some(w) = app.get_webview_window("notch") {
            let _ = w.close();
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// The notch webview drives its own expand/collapse; the native window
/// must resize in lockstep (top-center anchored, growing downward).
#[tauri::command]
fn notch_resize(app: AppHandle, width: f64, height: f64) {
    #[cfg(target_os = "macos")]
    notch::resize(&app, width, height);
    #[cfg(not(target_os = "macos"))]
    let _ = (app, width, height);
}

/// Watch all agent log directories and rescan automatically when
/// anything changes, then notify every window. Directories that appear
/// later (an agent installed after launch) are picked up within a minute.
fn spawn_usage_watcher(handle: AppHandle) {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc::RecvTimeoutError;
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let Ok(mut watcher) = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            },
        ) else {
            return;
        };
        let mut watched: HashSet<PathBuf> = HashSet::new();
        let mut last_dir_check: Option<Instant> = None;
        loop {
            if last_dir_check.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                let before = watched.len();
                for dir in adapters::ALL.iter().flat_map(|a| (a.data_dirs)()) {
                    if !watched.contains(&dir)
                        && watcher.watch(&dir, RecursiveMode::Recursive).is_ok()
                    {
                        watched.insert(dir);
                    }
                }
                // A source appeared after the initial scan: pick it up.
                if last_dir_check.is_some() && watched.len() > before {
                    let _ = run_scan(&handle);
                }
                last_dir_check = Some(Instant::now());
            }
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(()) => {
                    // Debounce: wait for 2s of quiet, but rescan at least
                    // every 10s while an agent keeps writing.
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while Instant::now() < deadline
                        && rx.recv_timeout(Duration::from_secs(2)).is_ok()
                    {}
                    let _ = run_scan(&handle);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

/// Day rollover and display changes: at local midnight the menu bar and
/// every window move on to the new day; when the notch display appears,
/// disappears or changes scale, the notch window follows.
fn spawn_clock_and_display_monitor(handle: AppHandle) {
    std::thread::spawn(move || {
        let mut day = chrono::Local::now().date_naive();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let today = chrono::Local::now().date_naive();
            if today != day {
                day = today;
                update_tray_title(&handle);
                let _ = handle.emit("usage-updated", ());
            }
            #[cfg(target_os = "macos")]
            {
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = handle.run_on_main_thread(move || {
                    let info = objc2::MainThreadMarker::new()
                        .map(notch::detect)
                        .unwrap_or_default();
                    let _ = tx.send(info);
                });
                if let Ok(info) = rx.recv_timeout(Duration::from_secs(2)) {
                    let state = handle.state::<AppState>();
                    let changed = {
                        let mut current = lock(&state.notch_info);
                        let changed = *current != info;
                        *current = info;
                        changed
                    };
                    if changed {
                        sync_notch_window(&handle);
                        update_tray_title(&handle);
                        let _ = handle.emit("notch-info-changed", info);
                    }
                }
            }
        }
    });
}

#[tauri::command]
fn show_main_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PricingStatus {
    /// "online" when a downloaded LiteLLM table is in use, else the
    /// embedded "snapshot".
    source: &'static str,
    snapshot_date: &'static str,
    fetched_at_ms: Option<i64>,
    model_count: usize,
    last_error: Option<String>,
    refreshing: bool,
    /// Models with usage but no known price (counted as $0).
    unpriced_models: Vec<String>,
}

fn pricing_status(app: &AppHandle) -> Result<PricingStatus, String> {
    let state = app.state::<AppState>();
    let pricing = state.pricing();
    let unpriced_models = db::unpriced_models(&lock(&state.conn), &pricing)?;
    let rt = lock(&state.pricing_rt);
    Ok(PricingStatus {
        source: if pricing.fetched_at_ms.is_some() { "online" } else { "snapshot" },
        snapshot_date: pricing::SNAPSHOT_DATE,
        fetched_at_ms: pricing.fetched_at_ms,
        model_count: pricing.model_count(),
        last_error: rt.last_error.clone(),
        refreshing: rt.refreshing,
        unpriced_models,
    })
}

/// Download fresh prices, swap them in, and re-price stored usage. Runs
/// on a background thread; concurrent calls collapse into one.
fn refresh_pricing_blocking(app: &AppHandle) {
    let state = app.state::<AppState>();
    {
        let mut rt = lock(&state.pricing_rt);
        if rt.refreshing {
            return;
        }
        rt.refreshing = true;
    }
    if let Ok(status) = pricing_status(app) {
        let _ = app.emit("pricing-updated", status);
    }
    let result = PricingMap::refresh_online(&state.cache_dir);
    if result.is_ok() {
        let fresh = Arc::new(PricingMap::load(Some(state.cache_dir.clone())));
        *state.pricing.write().unwrap_or_else(|e| e.into_inner()) = fresh;
    }
    {
        let mut rt = lock(&state.pricing_rt);
        rt.refreshing = false;
        rt.last_error = result.err();
    }
    // The scan re-prices stored usage when the fingerprint changed.
    let _ = run_scan(app);
    if let Ok(status) = pricing_status(app) {
        let _ = app.emit("pricing-updated", status);
    }
}

#[tauri::command(async)]
fn get_pricing_status(app: AppHandle) -> Result<PricingStatus, String> {
    pricing_status(&app)
}

#[tauri::command]
async fn refresh_pricing(app: AppHandle) -> Result<PricingStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        refresh_pricing_blocking(&app);
        pricing_status(&app)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Background auto-refresh of LiteLLM pricing: on launch when the local
/// cache is older than a day, then re-checked every 6 hours.
fn spawn_pricing_auto_refresh(handle: AppHandle) {
    std::thread::spawn(move || loop {
        let state = handle.state::<AppState>();
        let stale = state
            .pricing()
            .fetched_at_ms
            .is_none_or(|at| {
                let age_ms = chrono::Utc::now().timestamp_millis() - at;
                age_ms < 0 || age_ms as u128 > PRICING_MAX_AGE.as_millis()
            });
        if stale {
            refresh_pricing_blocking(&handle);
        }
        std::thread::sleep(Duration::from_secs(6 * 3600));
    });
}

#[tauri::command(async)]
fn get_overview(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<aggregate::Overview, String> {
    aggregate::overview(&lock(&state.conn), since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command(async)]
fn get_daily(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::DailyRow>, String> {
    aggregate::daily(&lock(&state.conn), since_ms, until_ms, parse_mode(cost_mode))
}

#[tauri::command(async)]
fn get_hourly(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::DailyRow>, String> {
    aggregate::hourly(&lock(&state.conn), since_ms, until_ms, parse_mode(cost_mode))
}

fn mark_priced(state: &AppState, rows: &mut [aggregate::ModelRow]) {
    let pricing = state.pricing();
    for row in rows {
        row.priced = pricing.is_priced(&row.model);
    }
}

#[tauri::command(async)]
fn get_models(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::ModelRow>, String> {
    let mut rows = aggregate::models(&lock(&state.conn), since_ms, until_ms, parse_mode(cost_mode))?;
    mark_priced(&state, &mut rows);
    Ok(rows)
}

#[tauri::command(async)]
fn get_sessions(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<aggregate::SessionRow>, String> {
    aggregate::sessions(
        &lock(&state.conn),
        since_ms,
        until_ms,
        parse_mode(cost_mode),
        limit.unwrap_or(200),
    )
}

#[tauri::command(async)]
fn get_projects(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    cost_mode: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<aggregate::ProjectRow>, String> {
    aggregate::projects(
        &lock(&state.conn),
        since_ms,
        until_ms,
        parse_mode(cost_mode),
        limit.unwrap_or(20),
    )
}

#[tauri::command(async)]
fn get_blocks(
    state: tauri::State<AppState>,
    since_ms: Option<i64>,
    cost_mode: Option<String>,
    agent: Option<String>,
) -> Result<Vec<aggregate::Block>, String> {
    aggregate::blocks(
        &lock(&state.conn),
        since_ms,
        parse_mode(cost_mode),
        5.0,
        agent.as_deref(),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceInfo {
    agent: String,
    dirs: Vec<String>,
    file_count: usize,
}

#[tauri::command(async)]
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

#[tauri::command(async)]
fn get_session_models(
    state: tauri::State<AppState>,
    agent: String,
    session_id: String,
    cost_mode: Option<String>,
) -> Result<Vec<aggregate::ModelRow>, String> {
    let mut rows =
        aggregate::session_models(&lock(&state.conn), &agent, &session_id, parse_mode(cost_mode))?;
    mark_priced(&state, &mut rows);
    Ok(rows)
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
    tauri::Builder::default()
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
            #[cfg(target_os = "macos")]
            let notch_info = notch::detect(
                objc2::MainThreadMarker::new()
                    .expect("tauri setup runs on the main thread"),
            );
            #[cfg(not(target_os = "macos"))]
            let notch_info = NotchInfo::default();
            app.manage(AppState {
                conn: Mutex::new(conn),
                pricing: RwLock::new(Arc::new(pricing)),
                pricing_rt: Mutex::new(PricingRuntime::default()),
                cache_dir: data_dir,
                settings: Mutex::new(settings),
                scan_lock: Mutex::new(()),
                notch_info: Mutex::new(notch_info),
            });
            setup_tray(app)?;
            sync_notch_window(app.handle());
            update_tray_title(app.handle());
            spawn_pricing_auto_refresh(app.handle().clone());
            spawn_usage_watcher(app.handle().clone());
            spawn_clock_and_display_monitor(app.handle().clone());
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
            get_cost_mode,
            set_cost_mode,
            set_language,
            get_notch_info,
            get_notch_enabled,
            set_notch_enabled,
            notch_resize,
            get_pricing_status,
            refresh_pricing,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
