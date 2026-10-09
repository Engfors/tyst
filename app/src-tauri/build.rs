/// Every app command, so Tauri checks each call against the window's capability
/// (`capabilities/*.json`) instead of allowing all app commands from every window.
const COMMANDS: &[&str] = &[
    "app_state",
    "audio_test",
    "config_get",
    "config_set",
    "dictation_cancel",
    "dictation_copy",
    "dictation_cycle_language",
    "dictation_discard",
    "dictation_edit",
    "dictation_info",
    "dictation_paste",
    "dictation_setup_keyboard",
    "dictation_setup_shortcuts",
    "dictation_state",
    "dictation_stop",
    "dictation_toggle",
    "install_window_rule",
    "meeting_preview",
    "meeting_prompt_answer",
    "meeting_save",
    "meeting_set_language",
    "meeting_start",
    "meeting_stop",
    "meeting_toggle_pause",
    "meeting_window_compact",
    "meeting_window_give_back_focus",
    "meeting_window_hide",
    "models_cancel",
    "models_fetch",
    "models_installed",
    "models_status",
    "models_verify",
    "onboarding_finish",
    "open_notices",
    "open_path",
    "open_transcripts_folder",
    "pick_folder",
    "reveal_path",
    "show_settings",
    "updates_check",
    "updates_open_release",
    "updates_state",
    "vocabulary_boost_ready",
    "vocabulary_export",
    "vocabulary_get",
    "vocabulary_import",
    "vocabulary_set",
];

fn main() {
    // The page must come from the app itself: a release build without `custom-protocol` loads
    // `devUrl` (http://localhost:5173), and whatever serves that port would drive the app.
    // `is_dev()` is true when `tauri` is built without it (`tauri build` turns it on).
    if std::env::var("PROFILE").as_deref() == Ok("release") && tauri_build::is_dev() {
        panic!("release builds need the `custom-protocol` feature; build with `npm run tauri build`");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build");
}
