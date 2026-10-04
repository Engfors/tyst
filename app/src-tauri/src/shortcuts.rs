//! Global shortcuts (SPEC 8.4, 10): dictation (default Ctrl+Å on Linux, Cmd+Å on macOS) and
//! the meeting toggle (Ctrl+Shift+Å / Cmd+Shift+Å, SPEC 15 q3).
//!
//! Linux: the XDG GlobalShortcuts portal, which works on Wayland; the desktop shows its own
//! dialog to confirm the keys the first time and lets the user change them in its settings
//! (KDE: System Settings › Keyboard › Shortcuts › Tyst). The portal reports presses and
//! releases, so push-to-talk works. macOS: Tauri's global-shortcut plugin, by key position
//! (`BracketLeft` is the Å key on a Swedish keyboard).

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::dictation::{self, Cmd};
use crate::state::{self, AppState};

pub const DICTATE: &str = "dictate";
pub const MEETING: &str = "toggle-meeting";
/// The app id the portal knows Tyst by (matches the bundle identifier and the .desktop file).
#[cfg(target_os = "linux")]
pub const APP_ID: &str = "com.engfors.tyst";

#[derive(Debug, Clone, Default, Serialize)]
pub struct ShortcutStatus {
    /// Shortcut id -> what the desktop shows for it ("Ctrl+Å"); empty when unassigned.
    pub bound: Vec<(String, String)>,
    /// Why the shortcuts are not available, if they are not.
    pub error: Option<String>,
    /// Waiting for the desktop (or the user's confirmation).
    pub pending: bool,
}

#[derive(Default)]
pub struct Shortcuts {
    status: Mutex<ShortcutStatus>,
    /// Ends the running portal session (Linux).
    #[cfg(target_os = "linux")]
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

pub fn status(app: &AppHandle) -> ShortcutStatus {
    app.state::<Shortcuts>().status.lock().expect("shortcut status lock").clone()
}

fn set_status(app: &AppHandle, f: impl FnOnce(&mut ShortcutStatus)) {
    f(&mut app.state::<Shortcuts>().status.lock().expect("shortcut status lock"));
}

fn on_meeting_shortcut(app: &AppHandle) {
    let app = app.clone();
    // Not on the shortcut task: starting and stopping take locks and may block briefly.
    std::thread::spawn(move || {
        let r = match app.state::<AppState>().phase() {
            state::Phase::Recording | state::Phase::Paused => {
                state::stop_meeting_in_background(&app);
                Ok(())
            }
            state::Phase::Starting => Ok(()),
            _ => state::start_meeting(&app),
        };
        if let Err(e) = r {
            crate::notify_error(&app, &e);
        }
    });
}

/// Registers the shortcuts (again), replacing earlier ones. Does nothing when dictation is off,
/// apart from the meeting shortcut.
pub fn register(app: &AppHandle) {
    let enabled = app.state::<AppState>().config().dictation.enabled;
    set_status(app, |s| *s = ShortcutStatus { pending: true, ..Default::default() });
    #[cfg(target_os = "linux")]
    linux::register(app, enabled);
    #[cfg(target_os = "macos")]
    macos::register(app, enabled);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = enabled;
        set_status(app, |s| {
            s.pending = false;
            s.error = Some("Global shortcuts are not available on this platform.".into());
        });
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use tyst_platform::portal::{self, ShortcutEvent, ShortcutSpec};

    use super::*;

    pub fn register(app: &AppHandle, dictation: bool) {
        let mut specs = vec![ShortcutSpec {
            id: MEETING.into(),
            description: "Start or stop meeting transcription".into(),
            preferred: Some("CTRL+SHIFT+aring".into()),
        }];
        if dictation {
            specs.insert(
                0,
                ShortcutSpec {
                    id: DICTATE.into(),
                    description: "Dictate (tap to start and stop, hold to talk)".into(),
                    preferred: Some("CTRL+aring".into()),
                },
            );
        }
        if let Err(e) = install_desktop_file() {
            log::warn!("desktop file: {e}");
        }
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        if let Some(old) = app.state::<Shortcuts>().stop.lock().expect("shortcut stop lock").replace(stop_tx) {
            let _ = old.send(());
        }
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let app = handle;
            // Must come before any other portal call on the connection.
            if let Err(e) = portal::register_app(APP_ID).await {
                log::info!("portal app registration: {e} (older xdg-desktop-portal?)");
            }
            let bound_app = app.clone();
            let event_app = app.clone();
            let r = portal::run_shortcuts(
                &specs,
                move |bound| {
                    log::info!(
                        "global shortcuts bound: {}",
                        bound.iter().map(|b| format!("{}={}", b.id, b.trigger)).collect::<Vec<_>>().join(", ")
                    );
                    set_status(&bound_app, |s| {
                        s.pending = false;
                        s.error = None;
                        s.bound = bound.into_iter().map(|b| (b.id, b.trigger)).collect();
                    });
                },
                move |e| match e {
                    ShortcutEvent::Pressed { id, at } if id == DICTATE => dictation::send(&event_app, Cmd::Press(at)),
                    ShortcutEvent::Released { id, at } if id == DICTATE => {
                        dictation::send(&event_app, Cmd::Release(at))
                    }
                    ShortcutEvent::Pressed { id, .. } if id == MEETING => on_meeting_shortcut(&event_app),
                    ShortcutEvent::Changed(bound) => {
                        set_status(&event_app, |s| s.bound = bound.into_iter().map(|b| (b.id, b.trigger)).collect())
                    }
                    _ => {}
                },
                stop_rx,
            )
            .await;
            if let Err(e) = r {
                log::warn!("global shortcuts: {e}");
                set_status(&app, |s| {
                    s.pending = false;
                    s.error = Some(format!(
                        "{e}. Tyst needs the desktop's GlobalShortcuts portal (KDE Plasma 6, GNOME 48 or newer); \
                         meanwhile bind `tyst --dictate` to a key yourself."
                    ));
                });
            }
        });
    }

    /// `~/.local/share/applications/com.engfors.tyst.desktop` and its icon: the portal identifies
    /// host apps by their desktop file, and KDE lists the shortcuts under its name and icon. Written
    /// on every start, so it follows an AppImage that was moved or replaced.
    fn install_desktop_file() -> Result<(), String> {
        let exe = crate::appimage::launch_path().ok_or("no executable path")?;
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("no HOME")?;
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        let icon = data.join("icons/hicolor/128x128/apps").join(format!("{APP_ID}.png"));
        let mut changed = write_if_changed(&icon, ICON)?;
        let exec = crate::appimage::exec_arg(&exe)?;
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=Tyst\nComment=Local meeting transcription and dictation\n\
             Exec={exec} %U\nIcon={APP_ID}\nTerminal=false\nCategories=Office;\n\
             X-GNOME-UsesNotifications=false\n"
        );
        changed |= write_if_changed(&data.join("applications").join(format!("{APP_ID}.desktop")), body.as_bytes())?;
        if changed && tyst_platform::kwin::is_kde() {
            // KDE's settings read names and icons from its service cache, which does not always
            // notice a new desktop file or icon on its own.
            match crate::appimage::host_command("kbuildsycoca6").output() {
                Ok(o) if o.status.success() => log::info!("KDE service cache refreshed"),
                Ok(o) => log::warn!("kbuildsycoca6 exited with {}", o.status),
                Err(e) => log::warn!("kbuildsycoca6: {e}"),
            }
        }
        Ok(())
    }

    const ICON: &[u8] = include_bytes!("../icons/128x128.png");

    /// Writes `bytes` unless the file already holds them; true if it wrote.
    fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> Result<bool, String> {
        if std::fs::read(path).is_ok_and(|old| old == bytes) {
            return Ok(false);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    use super::*;

    pub fn register(app: &AppHandle, dictation: bool) {
        let cfg = app.state::<AppState>().config().dictation;
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        let mut bound = Vec::new();
        let mut errors = Vec::new();
        let mut add = |id: &'static str, accel: &str| {
            let r = gs.on_shortcut(accel, move |app, _shortcut, event| match (id, event.state) {
                (DICTATE, ShortcutState::Pressed) => dictation::send(app, Cmd::Press(std::time::Instant::now())),
                (DICTATE, ShortcutState::Released) => dictation::send(app, Cmd::Release(std::time::Instant::now())),
                (MEETING, ShortcutState::Pressed) => on_meeting_shortcut(app),
                _ => {}
            });
            match r {
                Ok(()) => bound.push((id.to_string(), accel.to_string())),
                // Another app holds it, or it does not parse: warn (SPEC 10.1 conflict warning).
                Err(e) => errors.push(format!("{accel}: {e}")),
            }
        };
        if dictation {
            add(DICTATE, &cfg.shortcut);
        }
        add(MEETING, &cfg.meeting_shortcut);
        set_status(app, |s| {
            s.pending = false;
            s.bound = bound;
            s.error = if errors.is_empty() {
                None
            } else {
                Some(format!("Could not register {}. Pick another shortcut.", errors.join("; ")))
            };
        });
    }
}
