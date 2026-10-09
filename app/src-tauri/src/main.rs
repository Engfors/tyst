//! Tyst desktop app (SPEC 4, Phases 2-3): tray icon, floating meeting window, dictation pill,
//! onboarding and settings on top of `tyst-runtime`. Never logs transcript text or audio.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
mod appimage;
#[cfg(target_os = "linux")]
mod autostart;
mod commands;
mod config;
mod desktop;
mod detect;
mod dictation;
#[cfg(target_os = "linux")]
mod kwin;
mod secrets;
mod session;
mod shortcuts;
mod sources;
mod state;
mod tray;
mod updates;
mod windows;

use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tyst_core::journal;
use tyst_core::markdown;

use crate::config::Config;
use crate::state::AppState;

fn main() {
    // WebKitGTK's DMA-BUF renderer kills the app on some Wayland setups (notably NVIDIA) with
    // "Error 71 (Protocol error) dispatching to Wayland display". Our windows are small, so the
    // shared-memory path costs nothing noticeable. An explicit setting by the user wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: still single-threaded; nothing has read the environment yet.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let config = Config::load();

    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::Builder::new().build());
    let app = builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second launch passes its command line to the running app.
            if !handle_args(app, &args) {
                match app.state::<AppState>().phase() {
                    state::Phase::Idle => windows::show_settings(app, None),
                    _ => windows::show_meeting(app),
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new(config))
        .manage(commands::FetchState::default())
        .manage(desktop::Desktop::default())
        .manage(shortcuts::Shortcuts::default())
        .manage(secrets::KeyboardToken::default())
        .manage(updates::Updates::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_state,
            commands::meeting_start,
            commands::meeting_stop,
            commands::meeting_toggle_pause,
            commands::meeting_set_language,
            commands::meeting_preview,
            commands::meeting_save,
            commands::meeting_window_hide,
            commands::meeting_window_compact,
            commands::meeting_window_give_back_focus,
            commands::open_path,
            commands::reveal_path,
            commands::open_transcripts_folder,
            commands::open_notices,
            commands::config_get,
            commands::config_set,
            commands::pick_folder,
            commands::models_status,
            commands::models_fetch,
            commands::models_cancel,
            commands::models_verify,
            commands::models_installed,
            commands::vocabulary_get,
            commands::vocabulary_set,
            commands::vocabulary_import,
            commands::vocabulary_export,
            commands::vocabulary_boost_ready,
            commands::audio_test,
            commands::onboarding_finish,
            commands::show_settings,
            commands::install_window_rule,
            commands::dictation_state,
            commands::dictation_toggle,
            commands::dictation_stop,
            commands::dictation_cancel,
            commands::dictation_paste,
            commands::dictation_edit,
            commands::dictation_copy,
            commands::dictation_discard,
            commands::dictation_cycle_language,
            commands::dictation_info,
            commands::dictation_setup_shortcuts,
            commands::dictation_setup_keyboard,
            commands::meeting_prompt_answer,
            commands::updates_state,
            commands::updates_check,
            commands::updates_open_release,
        ])
        .setup(|app| {
            // Menu bar app: no Dock icon (SPEC 8.2).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let handle = app.handle().clone();
            dictation::init(&handle);
            tray::build(&handle)?;
            #[cfg(target_os = "macos")]
            windows::track_front_app();
            #[cfg(target_os = "linux")]
            if let Err(e) = kwin::install_rule() {
                log::warn!("KWin rule: {e}");
            }
            let cfg = handle.state::<AppState>().config();
            shortcuts::register(&handle);
            windows::prepare_pill(&handle);
            {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    handle.state::<secrets::KeyboardToken>().migrate(&handle).await;
                    updates::forget_legacy_token(&handle).await;
                });
            }
            if !cfg.onboarded {
                windows::show_onboarding(&handle);
            } else {
                // Rewrites the login entry, so it follows an AppImage that was moved or replaced.
                if cfg.launch_at_login {
                    commands::apply_autostart(&handle, true);
                }
                if tyst_runtime::fetch::installed(&tyst_runtime::fetch::DEFAULT_MODELS, &cfg.models_dir()) {
                    AppState::preload(&handle);
                }
                let args: Vec<String> = std::env::args().collect();
                handle_args(&handle, &args);
                start_background(&handle);
                std::thread::spawn(move || offer_recovery(&handle));
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("building the Tauri app");

    app.run(|app, event| match event {
        // Closing the last window keeps the tray app running; only Quit exits.
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            state::shutdown(app);
            let _ = app.state::<AppState>().config().save();
        }
        _ => {}
    });
}

/// Background tasks of an onboarded app: update check, meeting detection, idle model unload. Each
/// reads the settings on every pass, so they run whether or not their option is on.
pub(crate) fn start_background(app: &AppHandle) {
    updates::start(app);
    detect::start(app);
    AppState::unload_when_idle(app);
}

/// Command-line actions, for desktop shortcuts and scripts: `tyst --toggle-meeting` starts or
/// stops a meeting (in the running instance if there is one), `--pause` pauses or resumes,
/// `--dictate` starts or stops dictation, `--show-meeting` and `--settings` open windows.
/// Returns whether an action was found.
fn handle_args(app: &AppHandle, args: &[String]) -> bool {
    let mut handled = false;
    for a in args.iter().skip(1) {
        let r = match a.as_str() {
            "--toggle-meeting" => state::toggle_meeting(app),
            "--pause" => state::toggle_pause(app),
            "--dictate" => {
                dictation::send(app, dictation::Cmd::Toggle);
                Ok(())
            }
            "--show-meeting" => {
                windows::show_meeting(app);
                Ok(())
            }
            "--settings" => {
                windows::show_settings(app, None);
                Ok(())
            }
            _ => continue,
        };
        handled = true;
        if let Err(e) = r {
            log::error!("{a}: {e}");
            notify_error(app, &e);
        }
    }
    handled
}

/// Crash recovery (SPEC 6.6): every journal left by a session that never saved offers to become
/// a Markdown file.
fn offer_recovery(app: &AppHandle) {
    let Some(dir) = app.state::<AppState>().config().transcripts_dir else { return };
    let orphans = match journal::find_orphans(&dir) {
        Ok(o) => o,
        Err(e) => {
            log::error!("looking for journals: {e}");
            return;
        }
    };
    for path in orphans {
        let session = match journal::recover(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("unreadable journal {}: {e}", path.display());
                match journal::set_aside(&path) {
                    Ok(bad) => log::warn!("journal kept as {}", bad.display()),
                    Err(e) => log::error!("could not set the journal aside: {e}"),
                }
                continue;
            }
        };
        let when = session.info.started_at.format("%Y-%m-%d %H:%M");
        let recover = app
            .dialog()
            .message(format!(
                "Tyst found an unsaved meeting from {when} ({}). Recover it as a Markdown file?",
                match session.segments.len() {
                    1 => "1 passage".to_string(),
                    n => format!("{n} passages"),
                }
            ))
            .title("Recover unsaved meeting")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom("Recover".into(), "Not now".into()))
            .blocking_show();
        if !recover {
            continue;
        }
        match markdown::save(&session, &dir) {
            Ok(saved) => {
                if let Err(e) = std::fs::remove_file(&path) {
                    log::error!("removing recovered journal: {e}");
                }
                log::info!("recovered meeting {}", session.info.id);
                let _ = app.opener_reveal(&saved);
            }
            Err(e) => notify_error(app, &format!("Could not save the recovered meeting: {e}")),
        }
    }
}

trait Reveal {
    fn opener_reveal(&self, path: &std::path::Path) -> Result<(), String>;
}

impl Reveal for AppHandle {
    fn opener_reveal(&self, path: &std::path::Path) -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        self.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
    }
}

/// Shows an error to the user as a native dialog.
pub fn notify_error(app: &AppHandle, message: &str) {
    app.dialog().message(message).title("Tyst").kind(MessageDialogKind::Error).show(|_| {});
}
