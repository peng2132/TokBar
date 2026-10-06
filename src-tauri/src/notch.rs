//! macOS notch companion window.
//!
//! A black bar that fuses with the physical notch of 2021+ MacBooks and
//! shows today's usage in its two "wings"; clicking expands a detail
//! panel (Dynamic Island style). The window sits one level above the
//! menu bar and only ever grows downward — its top edge stays pinned to
//! the screen top so the fusion illusion never breaks.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSScreen, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::NotchInfo;

/// Idle margin beyond the notch, logical px per side. At rest the bar is
/// visually just the notch (plus a live dot); numbers appear on hover and
/// the webview resizes the window itself. Mirrors IDLE_WING in
/// src/pages/NotchBar.tsx.
const WING: f64 = 14.0;

/// First screen with a top safe-area inset, i.e. the built-in notched
/// display. `respondsToSelector` guards macOS < 12 where the API is missing.
fn notch_screen(mtm: MainThreadMarker) -> Option<objc2::rc::Retained<NSScreen>> {
    use objc2::runtime::NSObjectProtocol;
    NSScreen::screens(mtm)
        .iter()
        .find(|s| {
            s.respondsToSelector(objc2::sel!(safeAreaInsets))
                && s.safeAreaInsets().top > 0.0
        })
}

/// Measure the notch geometry; all-zero (`has_notch: false`) without one.
pub fn detect(mtm: MainThreadMarker) -> NotchInfo {
    let Some(screen) = notch_screen(mtm) else {
        return NotchInfo::default();
    };
    let frame = screen.frame();
    let left = screen.auxiliaryTopLeftArea();
    let right = screen.auxiliaryTopRightArea();
    NotchInfo {
        has_notch: true,
        notch_width: frame.size.width - left.size.width - right.size.width,
        bar_height: screen.safeAreaInsets().top,
        screen_width: frame.size.width,
    }
}

/// Create the companion window (idempotent; no-op without a notch). An
/// existing window is re-pinned to the notch screen at its idle size, for
/// when the display configuration changed; the webview then re-syncs its
/// own size from the `notch-info-changed` event.
pub fn create_window(app: &tauri::AppHandle, info: NotchInfo) {
    if !info.has_notch {
        return;
    }
    if app.get_webview_window("notch").is_some() {
        resize(app, info.notch_width + WING * 2.0, info.bar_height);
        return;
    }
    let handle = app.clone();
    // Window construction and NSWindow surgery must both happen on the
    // main thread; commands run on the async runtime.
    let _ = app.run_on_main_thread(move || {
        let width = info.notch_width + WING * 2.0;
        let height = info.bar_height;
        let win = WebviewWindowBuilder::new(
            &handle,
            "notch",
            WebviewUrl::App("index.html".into()),
        )
        .title("TokBar Notch")
        .inner_size(width, height)
        .decorations(false)
        .resizable(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .accept_first_mouse(true)
        .focused(false)
        .build();
        let Ok(win) = win else { return };
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Ok(ptr) = win.ns_window() {
            let ns: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
            // One level above the menu bar (24), so the bar may overlap
            // the menu-bar row that surrounds the physical notch.
            ns.setLevel(NSStatusWindowLevel);
            ns.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            ns.setMovable(false);
            place(ns, mtm, width, height);
        }
    });
}

/// Resize keeping the top-center anchor; called by the webview when the
/// detail panel expands or collapses.
pub fn resize(app: &tauri::AppHandle, width: f64, height: f64) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(win) = handle.get_webview_window("notch") else {
            return;
        };
        if let Ok(ptr) = win.ns_window() {
            let ns: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
            place(ns, mtm, width, height);
        }
    });
}

/// Pin the window to the top-center of the notch screen at the given
/// logical size (AppKit coordinates have a bottom-left origin, hence the
/// `screen top - height` math).
fn place(ns: &NSWindow, mtm: MainThreadMarker, width: f64, height: f64) {
    let Some(screen) = notch_screen(mtm) else { return };
    let f = screen.frame();
    let x = f.origin.x + (f.size.width - width) / 2.0;
    let y = f.origin.y + f.size.height - height;
    ns.setFrame_display(
        NSRect {
            origin: NSPoint { x, y },
            size: NSSize { width, height },
        },
        true,
    );
}
