//! Menu bar / system tray (SPEC 8.2): the app's only permanent presence (no Dock icon, no
//! taskbar entry). The icon shows idle, recording, paused, dictating or error.

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};
use tyst_core::router::LanguageMode;

use crate::dictation;
use crate::state::{self, AppState, Phase};
use crate::updates::{self, Updates};
use crate::windows;

pub struct Tray {
    icon: TrayIcon,
    start_stop: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    show: MenuItem<Wry>,
    dictate: MenuItem<Wry>,
    update: MenuItem<Wry>,
    lang: [CheckMenuItem<Wry>; 3],
    last: Mutex<Option<(Phase, bool, bool, bool)>>,
}

const MODES: [LanguageMode; 3] = [LanguageMode::Auto, LanguageMode::Swedish, LanguageMode::English];

fn icon(name: &str) -> Image<'static> {
    let bytes: &'static [u8] = match name {
        "recording" => include_bytes!("../icons/tray-recording.png"),
        "paused" => include_bytes!("../icons/tray-paused.png"),
        "error" => include_bytes!("../icons/tray-error.png"),
        "dictating" => include_bytes!("../icons/tray-dictating.png"),
        "update" => include_bytes!("../icons/tray-update.png"),
        _ => include_bytes!("../icons/tray-idle.png"),
    };
    Image::from_bytes(bytes).expect("tray icon is a valid PNG")
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let start_stop = MenuItem::with_id(app, "start_stop", "Start meeting transcription", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause", false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show meeting window", true, None::<&str>)?;
    let dictate = MenuItem::with_id(app, "dictate", "Start dictation", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "updates", "Check for updates…", true, None::<&str>)?;
    let lang = [
        CheckMenuItem::with_id(app, "lang_auto", "Auto", true, true, None::<&str>)?,
        CheckMenuItem::with_id(app, "lang_sv", "Svenska", true, false, None::<&str>)?,
        CheckMenuItem::with_id(app, "lang_en", "English", true, false, None::<&str>)?,
    ];
    let lang_menu = SubmenuBuilder::new(app, "Language").items(&[&lang[0], &lang[1], &lang[2]]).build()?;
    let menu: Menu<Wry> = MenuBuilder::new(app)
        .items(&[&start_stop, &pause, &show])
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&dictate)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&lang_menu)
        .item(&MenuItem::with_id(app, "open_folder", "Open transcripts folder", true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?)
        .item(&update)
        .item(&MenuItem::with_id(app, "about", "About Tyst", true, None::<&str>)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItem::with_id(app, "quit", "Quit Tyst", true, None::<&str>)?)
        .build()?;
    let icon = TrayIconBuilder::with_id("tyst")
        .icon(icon("idle"))
        .tooltip("Tyst")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .build(app)?;
    app.manage(Tray { icon, start_stop, pause, show, dictate, update, lang, last: Mutex::new(None) });
    refresh(app);
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    let result = match id {
        "start_stop" => match app.state::<AppState>().phase() {
            Phase::Recording | Phase::Paused => {
                state::stop_meeting_in_background(app);
                Ok(())
            }
            Phase::Idle | Phase::Naming => state::start_meeting(app),
            Phase::Starting => Ok(()),
        },
        "pause" => state::toggle_pause(app),
        "dictate" => {
            dictation::send(app, dictation::Cmd::Toggle);
            Ok(())
        }
        "show" => {
            windows::toggle_meeting(app);
            Ok(())
        }
        "lang_auto" | "lang_sv" | "lang_en" => {
            let mode = match id {
                "lang_sv" => LanguageMode::Swedish,
                "lang_en" => LanguageMode::English,
                _ => LanguageMode::Auto,
            };
            state::set_language(app, mode);
            Ok(())
        }
        "open_folder" => crate::commands::open_transcripts_folder(app.clone()),
        "settings" => {
            windows::show_settings(app, None);
            Ok(())
        }
        "updates" => {
            windows::show_settings(app, Some("updates"));
            if !app.state::<Updates>().available() {
                let app = app.clone();
                std::thread::spawn(move || updates::check(&app));
            }
            Ok(())
        }
        "about" => {
            windows::show_settings(app, Some("about"));
            Ok(())
        }
        "quit" => {
            state::shutdown(app);
            app.exit(0);
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        log::error!("tray action {id}: {e}");
        crate::notify_error(app, &e);
    }
    refresh(app);
}

/// Updates labels, checkmarks and the icon to the current state.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else { return };
    let st = app.state::<AppState>();
    let phase = st.phase();
    let warning = st.snapshot().warning.is_some() && phase == Phase::Idle;
    let language = st.language();
    for (item, mode) in tray.lang.iter().zip(MODES) {
        let _ = item.set_checked(mode == language);
    }
    let visible = windows::meeting_visible(app);
    let _ = tray.show.set_text(if visible { "Hide meeting window" } else { "Show meeting window" });
    let dictating = dictation::phase(app).recording();
    let _ = tray.dictate.set_text(if dictating { "Stop dictation" } else { "Start dictation" });
    let update = app.state::<Updates>().view(app);
    let _ = tray.update.set_text(match (&update.latest, update.available) {
        (Some(r), true) => format!("Update available: Tyst {}…", r.version),
        _ => "Check for updates…".to_string(),
    });
    let mut last = tray.last.lock().expect("tray lock");
    if *last == Some((phase, warning, dictating, update.available)) {
        return;
    }
    *last = Some((phase, warning, dictating, update.available));
    let (label, pause, pause_enabled, icon_name, tip) = match phase {
        Phase::Idle | Phase::Naming => ("Start meeting transcription", "Pause", false, "idle", "Tyst"),
        Phase::Starting => ("Starting…", "Pause", false, "recording", "Tyst: starting"),
        Phase::Recording => ("Stop meeting transcription", "Pause", true, "recording", "Tyst: recording"),
        Phase::Paused => ("Stop meeting transcription", "Resume", true, "paused", "Tyst: paused"),
    };
    let (icon_name, tip) = if dictating { ("dictating", "Tyst: dictating") } else { (icon_name, tip) };
    let (icon_name, tip) =
        if update.available && icon_name == "idle" { ("update", "Tyst: update available") } else { (icon_name, tip) };
    let icon_name = if warning { "error" } else { icon_name };
    let _ = tray.start_stop.set_text(label);
    let _ = tray.start_stop.set_enabled(phase != Phase::Starting);
    let _ = tray.pause.set_text(pause);
    let _ = tray.pause.set_enabled(pause_enabled);
    let _ = tray.icon.set_icon(Some(icon(icon_name)));
    let _ = tray.icon.set_tooltip(Some(tip));
}
