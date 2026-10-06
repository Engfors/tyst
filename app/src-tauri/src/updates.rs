//! Update notification (SPEC 9.7): on launch and every 24 hours, ask GitHub for the latest
//! release ([`tyst_runtime::update`]). A newer version shows as a dot on the tray icon, a tray
//! menu line and in Settings › Updates with its notes and a link to the release page. Nothing is
//! downloaded or installed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tyst_runtime::update::{self, Release};

use crate::secrets;
use crate::state::AppState;
use crate::tray;

pub const UPDATES_EVENT: &str = "tyst://updates";

/// The first check waits until the app has settled after launch.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateView {
    /// This build's version (or `TYST_PRETEND_VERSION`, for testing the notification).
    pub current: String,
    pub enabled: bool,
    pub checking: bool,
    /// Local time of the last finished check, `YYYY-MM-DD HH:MM`.
    pub last_checked: Option<String>,
    pub latest: Option<Release>,
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    view: Mutex<UpdateView>,
    last: Mutex<Option<Instant>>,
}

pub fn current_version() -> String {
    std::env::var("TYST_PRETEND_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

impl Updates {
    pub fn view(&self, app: &AppHandle) -> UpdateView {
        let cfg = app.state::<AppState>().config();
        let mut v = self.view.lock().expect("updates lock").clone();
        v.current = current_version();
        v.enabled = cfg.updates.check;
        v
    }

    /// Whether the tray should show a newer version.
    pub fn available(&self) -> bool {
        self.view.lock().expect("updates lock").available
    }
}

/// Starts the background schedule: a first check shortly after launch, then once a day while
/// checking is on.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("tyst-updates".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            loop {
                let due =
                    app.state::<Updates>().last.lock().expect("updates lock").is_none_or(|t| t.elapsed() >= INTERVAL);
                if due && app.state::<AppState>().config().updates.check {
                    check(&app);
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .expect("spawning the update thread");
}

/// Checks now (blocking: run off the UI thread).
pub fn check(app: &AppHandle) -> UpdateView {
    let updates = app.state::<Updates>();
    {
        let mut v = updates.view.lock().expect("updates lock");
        if v.checking {
            drop(v);
            return updates.view(app);
        }
        v.checking = true;
    }
    emit(app);
    let current = current_version();
    let agent = format!("Tyst/{}", env!("CARGO_PKG_VERSION"));
    let result = update::latest_release(update::REPO, &agent);
    {
        let mut v = updates.view.lock().expect("updates lock");
        v.checking = false;
        v.last_checked = Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string());
        match result {
            Ok(release) => {
                v.available = update::is_newer(&release.version, &current);
                log::info!("update check: latest {} (this is {current}), newer: {}", release.version, v.available);
                v.latest = Some(release);
                v.error = None;
            }
            Err(e) => {
                log::warn!("update check failed: {e}");
                v.error = Some(e.to_string());
            }
        }
    }
    *updates.last.lock().expect("updates lock") = Some(Instant::now());
    tray::refresh(app);
    emit(app);
    updates.view(app)
}

fn emit(app: &AppHandle) {
    let _ = app.emit(UPDATES_EVENT, app.state::<Updates>().view(app));
}

/// Removes the GitHub token versions up to 0.1.0 kept in the keychain for the private repository.
/// The check is anonymous now; the config remembers whether one was stored, so the keychain is
/// only asked when there is something to delete.
pub async fn forget_legacy_token(app: &AppHandle) {
    if !app.state::<AppState>().config().updates.github_token {
        return;
    }
    match secrets::set(secrets::LEGACY_GITHUB_TOKEN, None).await {
        Ok(()) => {
            app.state::<AppState>().update_config(|c| c.updates.github_token = false);
            log::info!("removed the old GitHub token from the keychain");
        }
        Err(e) => log::warn!("removing the old GitHub token: {e}"),
    }
}

/// Opens the latest release's page in the browser.
pub fn open_release(app: &AppHandle) -> Result<(), String> {
    let url = app.state::<Updates>().view.lock().expect("updates lock").latest.as_ref().map(|r| r.url.clone());
    let url = url.ok_or("No release found yet.")?;
    let expected = format!("https://github.com/{}/releases/", update::REPO);
    if !url.starts_with(&expected) {
        return Err("Unexpected release link.".into());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        crate::appimage::open_url(&url)
    }
    #[cfg(not(target_os = "linux"))]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
    }
}
