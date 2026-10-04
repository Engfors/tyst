//! Windows: the floating meeting window (SPEC 8.3), settings (8.6) and onboarding (8.5).
//!
//! The meeting window must never take keyboard focus from the meeting app (SPEC 8.1). It is
//! created unfocused and non-focusable, always on top, without decorations or a taskbar entry;
//! it only becomes focusable while the "Name this meeting" field waits for typing. On KDE
//! Wayland, where clients cannot keep themselves on top or position themselves, a KWin rule does
//! that job ([`crate::kwin`]).

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::config::WindowGeometry;
use crate::state::AppState;

pub const MEETING: &str = "meeting";
pub const SETTINGS: &str = "settings";
pub const ONBOARDING: &str = "onboarding";
/// Window title; the KWin rule matches it.
pub const MEETING_TITLE: &str = "Tyst Meeting";

/// Whether the meeting window was last shown or hidden. Tracked here because on Linux
/// `show()`/`hide()` are queued to the GTK loop, so `is_visible()` still reports the old state
/// right after the call and the tray label would be computed from it.
static MEETING_SHOWN: AtomicBool = AtomicBool::new(false);

fn meeting_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(MEETING) {
        return Ok(w);
    }
    let cfg = app.state::<AppState>().config();
    let mut b = WebviewWindowBuilder::new(app, MEETING, WebviewUrl::App("meeting.html".into()))
        .title(MEETING_TITLE)
        .inner_size(420.0, 260.0)
        .min_inner_size(260.0, 44.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(true)
        .focused(false)
        .focusable(false)
        .visible(false);
    // Restore the geometry saved for the primary display (the window is placed before it is
    // shown; per-display positions are applied once it knows its monitor).
    if let Some(g) = primary_geometry(app, &cfg.meetings.window) {
        b = b.position(g.x, g.y).inner_size(g.width, g.height);
    }
    let w = b.build()?;
    let handle = app.clone();
    w.on_window_event(move |e| match e {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => remember_geometry(&handle),
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide_meeting(&handle);
        }
        _ => {}
    });
    Ok(w)
}

fn primary_geometry(
    app: &AppHandle,
    saved: &std::collections::BTreeMap<String, WindowGeometry>,
) -> Option<WindowGeometry> {
    let name = app.primary_monitor().ok().flatten().and_then(|m| m.name().cloned())?;
    saved.get(&name).copied()
}

/// Keeps the window's position and size for the display it is on (written to disk when the
/// window hides).
fn remember_geometry(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MEETING) else { return };
    let Ok(scale) = w.scale_factor() else { return };
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) else { return };
    let Some(monitor) = w.current_monitor().ok().flatten().and_then(|m| m.name().cloned()) else { return };
    let pos = pos.to_logical::<f64>(scale);
    let size = size.to_logical::<f64>(scale);
    let g = WindowGeometry { x: pos.x, y: pos.y, width: size.width, height: size.height };
    app.state::<AppState>().config.lock().expect("config lock").meetings.window.insert(monitor, g);
}

pub fn show_meeting(app: &AppHandle) {
    match meeting_window(app) {
        Ok(w) => {
            // show() must not activate: the window is non-focusable until the name prompt.
            let _ = w.show();
            MEETING_SHOWN.store(true, Ordering::Release);
            crate::tray::refresh(app);
        }
        Err(e) => log::error!("meeting window: {e}"),
    }
}

pub fn hide_meeting(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MEETING) {
        let _ = w.hide();
        let _ = w.set_focusable(false);
    }
    MEETING_SHOWN.store(false, Ordering::Release);
    if let Err(e) = app.state::<AppState>().config().save() {
        log::error!("saving config: {e}");
    }
    crate::tray::refresh(app);
}

pub fn meeting_visible(app: &AppHandle) -> bool {
    MEETING_SHOWN.load(Ordering::Acquire) && app.get_webview_window(MEETING).is_some()
}

pub fn toggle_meeting(app: &AppHandle) {
    if meeting_visible(app) { hide_meeting(app) } else { show_meeting(app) }
}

/// Lets the meeting window take focus for the name prompt (SPEC 8.3, the one explicit exception
/// to "never steal focus"), and gives it back afterwards.
pub fn focus_for_typing(app: &AppHandle, on: bool) {
    let Some(w) = app.get_webview_window(MEETING) else { return };
    let _ = w.set_focusable(on);
    if on {
        let _ = w.set_focus();
    }
}

/// Grows or shrinks the window between the full and the one-line compact layout.
pub fn set_compact(app: &AppHandle, compact: bool) {
    let st = app.state::<AppState>();
    st.config.lock().expect("config lock").meetings.compact = compact;
    if let Some(w) = app.get_webview_window(MEETING) {
        let width = w
            .inner_size()
            .ok()
            .and_then(|s| w.scale_factor().ok().map(|f| s.to_logical::<f64>(f).width))
            .unwrap_or(420.0);
        let _ = w.set_size(LogicalSize::new(width, if compact { 44.0 } else { 260.0 }));
    }
}

fn show_simple(app: &AppHandle, label: &str, url: &str, title: &str, size: (f64, f64)) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.eval(format!("window.location.replace('{url}')"));
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(size.0, size.1)
        .min_inner_size(480.0, 400.0)
        .center()
        .build();
    if let Err(e) = built {
        log::error!("{label} window: {e}");
    }
}

/// Settings, optionally opened on a tab ("about", "models", …).
pub fn show_settings(app: &AppHandle, tab: Option<&str>) {
    let url = match tab {
        Some(t) => format!("settings.html#{t}"),
        None => "settings.html".into(),
    };
    show_simple(app, SETTINGS, &url, "Tyst Settings", (680.0, 540.0));
}

pub fn show_onboarding(app: &AppHandle) {
    show_simple(app, ONBOARDING, "onboarding.html", "Welcome to Tyst", (600.0, 520.0));
}

pub fn close(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
}
