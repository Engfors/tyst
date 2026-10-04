//! Meeting detection (SPEC 8.3, 9.4; off by default): when an app from the meeting-app list
//! starts using the microphone while Tyst is idle, the pill asks "Teams is using the microphone.
//! Transcribe this meeting?". Nothing records without the user's answer. Each microphone use is
//! asked about once; a dismissed one is not asked again until the app lets go of the mic.

use std::collections::HashSet;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::dictation::{self, Cmd};
use crate::state::{AppState, Phase};

const POLL: Duration = Duration::from_secs(2);

/// Whether this build can see other apps' microphone use.
pub const SUPPORTED: bool =
    cfg!(any(all(target_os = "linux", feature = "pipewire"), all(target_os = "macos", feature = "macos-tap")));

/// Starts the watcher thread. It only looks at the microphone while the setting is on.
pub fn start(app: &AppHandle) {
    if !SUPPORTED {
        return;
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name("tyst-detect".into())
        .spawn(move || run(app))
        .expect("spawning the detection thread");
}

#[cfg(any(all(target_os = "linux", feature = "pipewire"), all(target_os = "macos", feature = "macos-tap")))]
fn run(app: AppHandle) {
    use tyst_platform::mic_watch::MicWatcher;

    let mut watcher: Option<MicWatcher> = None;
    let mut seen: HashSet<u64> = HashSet::new();
    let mut retry_at = std::time::Instant::now();
    loop {
        std::thread::sleep(POLL);
        let cfg = app.state::<AppState>().config();
        if !cfg.meetings.detect {
            if watcher.take().is_some() {
                log::info!("meeting detection off");
            }
            seen.clear();
            continue;
        }
        if watcher.is_none() {
            if std::time::Instant::now() < retry_at {
                continue;
            }
            match MicWatcher::start() {
                Ok(w) => {
                    log::info!("meeting detection on");
                    // Apps already in a call when detection starts are asked about too.
                    watcher = Some(w);
                }
                Err(e) => {
                    log::warn!("meeting detection unavailable: {e}");
                    retry_at = std::time::Instant::now() + Duration::from_secs(60);
                    continue;
                }
            }
        }
        let Some(w) = &watcher else { continue };
        let users = w.users();
        seen.retain(|id| users.iter().any(|u| u.id == *id));
        let idle = app.state::<AppState>().phase() == Phase::Idle && dictation::phase(&app) == dictation::Phase::Idle;
        for u in users.iter().filter(|u| u.matches(&cfg.meetings.detect_apps)) {
            if seen.insert(u.id) && idle {
                log::info!("meeting app is using the microphone");
                dictation::send(&app, Cmd::Prompt(u.display_name().to_string()));
                break;
            }
        }
    }
}

#[cfg(not(any(all(target_os = "linux", feature = "pipewire"), all(target_os = "macos", feature = "macos-tap"))))]
fn run(_app: AppHandle) {
    let _ = (POLL, HashSet::<u64>::new());
}
