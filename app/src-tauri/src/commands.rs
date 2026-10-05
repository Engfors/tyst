//! Commands the UI calls (`invoke`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
#[cfg(not(target_os = "linux"))]
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tyst_core::models::{FileStatus, Manifest, PARAKEET};
use tyst_core::router::LanguageMode;
use tyst_core::transcript::Channel;
use tyst_core::vocabulary::{Replacement, VocabularyFile, VocabularyRules};
use tyst_runtime::fetch::{self, DEFAULT_MODELS, Progress};

use crate::config::{self, Config};
use crate::desktop::Desktop;
use crate::dictation::{self, Cmd};
use crate::secrets::KeyboardToken;
use crate::state::{self, AppState, Snapshot};
use crate::updates::{self, UpdateView, Updates};
use crate::{shortcuts, windows};

pub const MODELS_EVENT: &str = "tyst://models";

type CmdResult<T = ()> = Result<T, String>;

#[tauri::command]
pub fn app_state(app: AppHandle) -> Snapshot {
    app.state::<AppState>().snapshot()
}

#[tauri::command]
pub fn meeting_start(app: AppHandle) -> CmdResult {
    state::start_meeting(&app)
}

/// Stops capture; the final decodes run off the UI thread.
#[tauri::command]
pub async fn meeting_stop(app: AppHandle) -> CmdResult {
    tauri::async_runtime::spawn_blocking(move || state::stop_meeting(&app)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn meeting_toggle_pause(app: AppHandle) -> CmdResult {
    state::toggle_pause(&app)
}

#[tauri::command]
pub fn meeting_set_language(app: AppHandle, language: String) -> CmdResult {
    let mode = LanguageMode::parse(&language).ok_or("language must be auto, sv or en")?;
    state::set_language(&app, mode);
    Ok(())
}

#[tauri::command]
pub fn meeting_preview(app: AppHandle, title: Option<String>) -> Option<String> {
    state::preview_path(&app, title.as_deref().map(str::trim).filter(|t| !t.is_empty()))
}

#[tauri::command]
pub fn meeting_save(app: AppHandle, title: Option<String>) -> CmdResult<String> {
    state::save_meeting(&app, title)
}

#[tauri::command]
pub fn meeting_window_hide(app: AppHandle) {
    windows::hide_meeting(&app);
}

#[tauri::command]
pub fn meeting_window_compact(app: AppHandle, compact: bool) {
    windows::set_compact(&app, compact);
}

/// Opens a saved transcript (only files inside the transcripts folder).
#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> CmdResult {
    let path = transcript_path(&app, &path)?;
    open(&app, &path)
}

/// Shows a saved transcript in the file manager (only files inside the transcripts folder).
#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> CmdResult {
    let path = transcript_path(&app, &path)?;
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

fn transcript_path(app: &AppHandle, path: &str) -> CmdResult<std::path::PathBuf> {
    let dir = app.state::<AppState>().config().transcripts_dir.ok_or("No transcripts folder chosen yet.")?;
    inside(&dir, path)
}

/// `path` as an existing file or folder inside `dir` (after resolving `..` and links). URLs and
/// anything else are refused, so a webview cannot use the opener to reach the network or run
/// other files.
fn inside(dir: &std::path::Path, path: &str) -> CmdResult<std::path::PathBuf> {
    let refuse = || format!("Not a file in the transcripts folder: {path}");
    if path.contains("://") || !std::path::Path::new(path).is_absolute() {
        return Err(refuse());
    }
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let path = std::path::Path::new(path).canonicalize().map_err(|_| refuse())?;
    if path.starts_with(&dir) { Ok(path) } else { Err(refuse()) }
}

#[tauri::command]
pub fn open_transcripts_folder(app: AppHandle) -> CmdResult {
    let dir = app.state::<AppState>().config().transcripts_dir.ok_or("No transcripts folder chosen yet.")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open(&app, &dir)
}

/// Opens the third-party notices and licenses that ship with the app (Settings › About).
#[tauri::command]
pub fn open_notices(app: AppHandle) -> CmdResult {
    let file = app.path().resource_dir().map_err(|e| e.to_string())?.join("THIRD_PARTY_NOTICES.md");
    if !file.is_file() {
        return Err("The third-party notices are missing from this build.".into());
    }
    open(&app, &file)
}

/// Opens a file or folder with its default application. On Linux the application gets the host's
/// environment, not the AppImage's ([`crate::appimage`]).
fn open(app: &AppHandle, path: &std::path::Path) -> CmdResult {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        crate::appimage::open(path)
    }
    #[cfg(not(target_os = "linux"))]
    app.opener().open_path(path.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ConfigView {
    pub config: Config,
    pub config_dir: String,
    pub default_transcripts_dir: String,
    pub default_models_dir: String,
    pub system_audio_supported: bool,
    pub detect_supported: bool,
    /// Others can be limited to meeting apps (macOS).
    pub app_filter_supported: bool,
    pub version: String,
    pub platform: &'static str,
}

#[tauri::command]
pub fn config_get(app: AppHandle) -> ConfigView {
    let mut config = app.state::<AppState>().config();
    // The paste consent token never goes to a webview; `dictation_info` says whether one exists.
    config.dictation.keyboard_token = None;
    ConfigView {
        config,
        config_dir: config::config_dir().display().to_string(),
        default_transcripts_dir: config::default_transcripts_dir().display().to_string(),
        default_models_dir: tyst_core::models::default_models_dir().display().to_string(),
        system_audio_supported: crate::sources::system_audio().is_some(),
        detect_supported: crate::detect::SUPPORTED,
        app_filter_supported: crate::sources::FILTERS_APPS,
        version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS,
    }
}

/// Saves settings and applies what changed (launch at login, models).
#[tauri::command]
pub fn config_set(app: AppHandle, config: Config) -> CmdResult {
    let st = app.state::<AppState>();
    let old = st.config();
    if config.launch_at_login != old.launch_at_login {
        apply_autostart(&app, config.launch_at_login);
    }
    if config.threads != old.threads || config.models_dir != old.models_dir {
        st.unload_runtime();
    }
    // Window geometry, the paste consent token and the last dictation language are owned by
    // the app, not the settings form.
    let mut config = config;
    config.meetings.window = old.meetings.window.clone();
    config.dictation.keyboard_token = old.dictation.keyboard_token.clone();
    config.dictation.keyboard_access = old.dictation.keyboard_access;
    config.updates.github_token = old.updates.github_token;
    config.dictation.last_lang = old.dictation.last_lang;
    let rebind = config.dictation.enabled != old.dictation.enabled
        || config.dictation.shortcut != old.dictation.shortcut
        || config.dictation.meeting_shortcut != old.dictation.meeting_shortcut;
    config.save()?;
    *st.config.lock().expect("config lock") = config;
    st.emit_state(&app);
    if rebind {
        shortcuts::register(&app);
    }
    Ok(())
}

pub fn apply_autostart(app: &AppHandle, on: bool) {
    #[cfg(target_os = "linux")]
    let r = {
        let _ = app;
        crate::autostart::set(on)
    };
    #[cfg(not(target_os = "linux"))]
    let r = {
        let al = app.autolaunch();
        if on { al.enable() } else { al.disable() }
    };
    if let Err(e) = r {
        log::warn!("launch at login: {e}");
    }
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, current: Option<String>) -> Option<String> {
    let mut d = app.dialog().file();
    if let Some(c) = current {
        d = d.set_directory(c);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    d.pick_folder(move |p| {
        let _ = tx.send(p);
    });
    tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .ok()
        .flatten()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.display().to_string())
}

#[derive(Serialize)]
pub struct ModelRow {
    pub id: String,
    pub kind: String,
    pub version: String,
    pub license: String,
    pub size_mb: f64,
    /// installed, missing, partial or broken
    pub status: String,
    /// Not needed unless English is forced.
    pub optional: bool,
}

#[tauri::command]
pub fn models_status(app: AppHandle) -> CmdResult<Vec<ModelRow>> {
    let dir = app.state::<AppState>().config().models_dir();
    let manifest = Manifest::builtin();
    let mut rows = Vec::new();
    for (id, spec) in &manifest.models {
        let statuses =
            tyst_core::models::verify(spec, &dir, tyst_core::models::Check::Size).map_err(|e| e.to_string())?;
        let ok = statuses.iter().filter(|(_, s)| *s == FileStatus::Ok).count();
        let missing = statuses.iter().filter(|(_, s)| *s == FileStatus::Missing).count();
        let status = if fetch::installed(&[id.as_str()], &dir) {
            "installed"
        } else if missing == statuses.len() {
            "missing"
        } else if ok + missing == statuses.len() {
            "partial"
        } else {
            "broken"
        };
        rows.push(ModelRow {
            id: id.clone(),
            kind: spec.kind.clone(),
            version: spec.tagged_id(id),
            license: spec.license.clone(),
            size_mb: spec.total_size() as f64 / 1e6,
            status: status.into(),
            optional: !DEFAULT_MODELS.contains(&id.as_str()),
        });
    }
    Ok(rows)
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelsEvent {
    Progress { file: String, done: u64, total: u64, note: String },
    Done,
    Failed { message: String },
}

#[derive(Default)]
pub struct FetchState {
    cancel: Arc<AtomicBool>,
    running: AtomicBool,
}

/// Downloads models in the background (SPEC 5.2: only on the user's request), reporting on
/// `tyst://models`. `ids` empty means the default set; `["parakeet-v3-int8"]` adds English.
#[tauri::command]
pub fn models_fetch(app: AppHandle, ids: Vec<String>) -> CmdResult {
    let fs = app.state::<FetchState>();
    if fs.running.swap(true, Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    fs.cancel.store(false, Ordering::SeqCst);
    let cancel = fs.cancel.clone();
    let dir = app.state::<AppState>().config().models_dir();
    std::thread::spawn(move || {
        // Clears the running flag however this thread ends, panics included, so a crash in
        // the download never blocks the next one until a restart.
        let _running = DownloadRunning(app.clone());
        let ids: Vec<String> =
            if ids.is_empty() { DEFAULT_MODELS.iter().map(|s| s.to_string()).collect() } else { ids };
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let emit = |e: ModelsEvent| {
            let _ = app.emit(MODELS_EVENT, e);
        };
        let result = fetch::fetch(&refs, &dir, &cancel, &mut |p| {
            let (file, done, total, note) = match p {
                Progress::Model { id, bytes } => (id, 0, bytes, "starting".to_string()),
                Progress::Present { file } => (file, 1, 1, "present".into()),
                Progress::Downloading { file, done, total } => (file, done, total, "downloading".into()),
                Progress::Deriving { file } => (file, 0, 1, "optimizing".into()),
                Progress::Verified { file } => (file, 1, 1, "verified".into()),
            };
            emit(ModelsEvent::Progress { file, done, total, note });
        });
        drop(_running);
        match result {
            Ok(()) => {
                emit(ModelsEvent::Done);
                AppState::preload(&app);
            }
            Err(e) => {
                log::error!("model download: {e}");
                emit(ModelsEvent::Failed { message: e.to_string() });
            }
        }
    });
    Ok(())
}

struct DownloadRunning(AppHandle);

impl Drop for DownloadRunning {
    fn drop(&mut self) {
        self.0.state::<FetchState>().running.store(false, Ordering::SeqCst);
        if std::thread::panicking() {
            let _ =
                self.0.emit(MODELS_EVENT, ModelsEvent::Failed { message: "The download stopped unexpectedly.".into() });
        }
    }
}

#[tauri::command]
pub fn models_cancel(app: AppHandle) {
    app.state::<FetchState>().cancel.store(true, Ordering::SeqCst);
}

/// Checks sizes (and with `full`, SHA-256) of every installed file.
#[tauri::command]
pub async fn models_verify(app: AppHandle, full: bool) -> CmdResult<Vec<String>> {
    let dir = app.state::<AppState>().config().models_dir();
    tauri::async_runtime::spawn_blocking(move || {
        let manifest = Manifest::builtin();
        let mut problems = Vec::new();
        for (id, spec) in &manifest.models {
            let statuses = tyst_core::models::verify(
                spec,
                &dir,
                if full { tyst_core::models::Check::Full } else { tyst_core::models::Check::Stamp },
            )
            .map_err(|e| e.to_string())?;
            if id == PARAKEET && statuses.iter().all(|(_, s)| *s == FileStatus::Missing) {
                continue;
            }
            for (name, s) in statuses {
                if s != FileStatus::Ok {
                    problems.push(format!("{id}: {name}: {s:?}"));
                }
            }
        }
        Ok(problems)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn models_installed(app: AppHandle) -> bool {
    fetch::installed(&DEFAULT_MODELS, &app.state::<AppState>().config().models_dir())
}

/// The vocabulary as the UI sees it. `VocabularyFile` names its list `replacement` so the TOML reads
/// as `[[replacement]]` tables; over IPC that name would not match the UI's `replacements`, and serde
/// would silently drop the rules.
#[derive(Serialize, Deserialize)]
pub struct VocabularyDto {
    #[serde(default)]
    terms: Vec<String>,
    #[serde(default)]
    replacements: Vec<Replacement>,
}

impl From<VocabularyFile> for VocabularyDto {
    fn from(v: VocabularyFile) -> Self {
        Self { terms: v.terms, replacements: v.replacements }
    }
}

impl From<VocabularyDto> for VocabularyFile {
    fn from(v: VocabularyDto) -> Self {
        Self { terms: v.terms, replacements: v.replacements }
    }
}

#[tauri::command]
pub fn vocabulary_get() -> VocabularyDto {
    config::load_vocabulary().into()
}

#[tauri::command]
pub fn vocabulary_set(app: AppHandle, vocabulary: VocabularyDto) -> CmdResult {
    save_vocabulary(&app, &vocabulary.into())
}

fn save_vocabulary(app: &AppHandle, vocabulary: &VocabularyFile) -> CmdResult {
    config::save_vocabulary(vocabulary)?;
    app.state::<AppState>().set_vocabulary(VocabularyRules::new(vocabulary));
    Ok(())
}

/// Imports a vocabulary TOML file chosen by the user, replacing the current lists.
#[tauri::command]
pub async fn vocabulary_import(app: AppHandle) -> CmdResult<Option<VocabularyDto>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().add_filter("Vocabulary", &["toml"]).pick_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(path) = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| e.to_string())?
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    let file = tyst_core::vocabulary::load_file(&path).map_err(|e| e.to_string())?;
    save_vocabulary(&app, &file)?;
    Ok(Some(file.into()))
}

#[tauri::command]
pub async fn vocabulary_export(app: AppHandle) -> CmdResult<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().add_filter("Vocabulary", &["toml"]).set_file_name("tyst-vocabulary.toml").save_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(path): Option<PathBuf> = tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| e.to_string())?
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    config::write_toml(&path, &config::load_vocabulary())?;
    Ok(Some(path.display().to_string()))
}

#[derive(Serialize)]
pub struct AudioTest {
    pub peak: f32,
    pub text: String,
}

/// Records a few seconds and transcribes them (onboarding "Say something", permission check).
#[tauri::command]
pub async fn audio_test(app: AppHandle, channel: String, seconds: Option<f32>) -> CmdResult<AudioTest> {
    let channel = if channel == "others" { Channel::Others } else { Channel::Me };
    let seconds = seconds.unwrap_or(4.0).clamp(1.0, 15.0);
    tauri::async_runtime::spawn_blocking(move || state::audio_test(&app, channel, seconds))
        .await
        .map_err(|e| e.to_string())?
        .map(|(peak, text)| AudioTest { peak, text })
}

#[tauri::command]
pub fn onboarding_finish(app: AppHandle) -> CmdResult {
    let st = app.state::<AppState>();
    let cfg = {
        let mut cfg = st.config.lock().expect("config lock");
        cfg.onboarded = true;
        cfg.clone()
    };
    cfg.save()?;
    apply_autostart(&app, cfg.launch_at_login);
    windows::close(&app, windows::ONBOARDING);
    AppState::preload(&app);
    Ok(())
}

#[tauri::command]
pub fn show_settings(app: AppHandle, tab: Option<String>) {
    windows::show_settings(&app, tab.as_deref());
}

#[tauri::command]
pub fn dictation_state(app: AppHandle) -> Option<dictation::PillState> {
    dictation::pill_state(&app)
}

#[tauri::command]
pub fn dictation_toggle(app: AppHandle) {
    dictation::send(&app, Cmd::Toggle);
}

#[tauri::command]
pub fn dictation_stop(app: AppHandle) {
    dictation::send(&app, Cmd::Stop);
}

#[tauri::command]
pub fn dictation_cancel(app: AppHandle) {
    dictation::send(&app, Cmd::Cancel);
}

/// Pastes the (possibly edited) preview text.
#[tauri::command]
pub fn dictation_paste(app: AppHandle, text: Option<String>) {
    dictation::send(&app, Cmd::Paste(text));
}

/// The user edited the preview text (so the shortcut pastes the edited version).
#[tauri::command]
pub fn dictation_edit(app: AppHandle, text: String) {
    dictation::send(&app, Cmd::Edit(text));
}

#[tauri::command]
pub fn dictation_copy(app: AppHandle, text: String) {
    dictation::send(&app, Cmd::Copy(text));
}

#[tauri::command]
pub fn dictation_discard(app: AppHandle) {
    dictation::send(&app, Cmd::Discard);
}

#[tauri::command]
pub fn dictation_cycle_language(app: AppHandle) {
    dictation::send(&app, Cmd::CycleLanguage);
}

#[derive(Serialize)]
pub struct DictationInfo {
    pub shortcuts: shortcuts::ShortcutStatus,
    /// Linux: keyboard access for paste was granted before (a restore token is kept).
    pub keyboard_granted: bool,
    pub english_model: bool,
}

#[tauri::command]
pub fn dictation_info(app: AppHandle) -> DictationInfo {
    let cfg = app.state::<AppState>().config();
    DictationInfo {
        shortcuts: shortcuts::status(&app),
        keyboard_granted: cfg.dictation.keyboard_access || cfg.dictation.keyboard_token.is_some(),
        english_model: fetch::installed(&[PARAKEET], &cfg.models_dir()),
    }
}

/// Binds the shortcuts again (the desktop may ask to confirm them).
#[tauri::command]
pub fn dictation_setup_shortcuts(app: AppHandle) {
    shortcuts::register(&app);
}

/// Asks for keyboard access for paste now (Linux: the desktop's consent dialog).
#[tauri::command]
pub async fn dictation_setup_keyboard(app: AppHandle) -> CmdResult {
    let keyboard = app.state::<KeyboardToken>();
    let token = keyboard.get(&app).await;
    let token = app.state::<Desktop>().setup_keyboard(token).await?;
    keyboard.set(&app, token).await;
    Ok(())
}

/// The meeting prompt's Start or Dismiss (meeting detection).
#[tauri::command]
pub fn meeting_prompt_answer(app: AppHandle, start: bool) {
    dictation::send(&app, Cmd::PromptAnswer(start));
}

#[tauri::command]
pub fn updates_state(app: AppHandle) -> UpdateView {
    app.state::<Updates>().view(&app)
}

#[tauri::command]
pub async fn updates_check(app: AppHandle) -> Result<UpdateView, String> {
    tauri::async_runtime::spawn_blocking(move || updates::check(&app)).await.map_err(|e| e.to_string())
}

/// Stores (or with `None` removes) the GitHub token for the update check. The token goes to the
/// keychain only and is never sent back to the UI.
#[tauri::command]
pub async fn updates_set_token(app: AppHandle, token: Option<String>) -> Result<UpdateView, String> {
    updates::set_token(&app, token).await?;
    tauri::async_runtime::spawn_blocking(move || updates::check(&app)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn updates_open_release(app: AppHandle) -> CmdResult {
    updates::open_release(&app)
}

#[tauri::command]
pub fn install_window_rule() -> CmdResult {
    #[cfg(target_os = "linux")]
    return crate::kwin::install_rule();
    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opener_only_reaches_the_transcripts_folder() {
        let root = std::env::temp_dir().join(format!("tyst-open-{}", std::process::id()));
        let dir = root.join("Transcripts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(root.join("secret"), "x").unwrap();
        assert!(inside(&dir, dir.join("a.md").to_str().unwrap()).is_ok());
        assert!(inside(&dir, dir.join("../secret").to_str().unwrap()).is_err());
        assert!(inside(&dir, root.join("secret").to_str().unwrap()).is_err());
        assert!(inside(&dir, "https://example.com/x").is_err());
        assert!(inside(&dir, "a.md").is_err());
        assert!(inside(&dir, dir.join("missing.md").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn vocabulary_keeps_replacements_over_ipc() {
        // The shape `Vocabulary.svelte` sends and expects back.
        let json = r#"{"terms":["HashiCorp"],"replacements":[{"from":"hashi corp","to":"HashiCorp"}]}"#;
        let file: VocabularyFile = serde_json::from_str::<VocabularyDto>(json).unwrap().into();
        assert_eq!(file.replacements, vec![Replacement { from: "hashi corp".into(), to: "HashiCorp".into() }]);
        let back = serde_json::to_string(&VocabularyDto::from(file)).unwrap();
        assert_eq!(back, json);
    }
}
