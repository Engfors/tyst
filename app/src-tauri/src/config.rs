//! App settings (SPEC 8.6) as TOML in the config dir (SPEC 11): `~/.config/tyst/` on Linux,
//! `~/Library/Application Support/Tyst/` on macOS. The vocabulary lists live next to it in
//! `vocabulary.toml` (SPEC 9.3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tyst_core::transcript::SpeakerLabels;
use tyst_core::vocabulary::VocabularyFile;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Onboarding finished.
    pub onboarded: bool,
    /// Where Markdown transcripts (and the crash journal) go.
    pub transcripts_dir: Option<PathBuf>,
    /// Models directory; `None` is the platform default (SPEC 5.2).
    pub models_dir: Option<PathBuf>,
    /// Default session language: auto, sv or en.
    pub language: String,
    pub labels: SpeakerLabels,
    pub launch_at_login: bool,
    /// ONNX Runtime threads.
    pub threads: usize,
    /// Unload the models after this many minutes without a meeting or dictation; 0 keeps them
    /// loaded (SPEC 5.2).
    pub models_idle_minutes: u32,
    pub meetings: MeetingSettings,
    pub dictation: DictationSettings,
    pub updates: UpdateSettings,
}

/// Update notification (SPEC 9.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Ask GitHub for a newer release on launch and once a day.
    pub check: bool,
    /// A GitHub token is in the keychain (needed while the repository is private).
    pub github_token: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { check: true, github_token: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingSettings {
    /// Capture system audio as Others (off: Me only).
    pub system_audio: bool,
    /// macOS: Others records only the apps in `detect_apps` (SPEC 15 q7, on by default), so music
    /// and notification sounds stay out of the transcript.
    pub only_meeting_apps: bool,
    /// Remove the system audio's echo from the microphone (SPEC 6.4), for meetings on speakers.
    pub echo_cancellation: bool,
    /// Show the meeting window when recording starts.
    pub show_window_on_start: bool,
    /// Start in the compact one-line mode.
    pub compact: bool,
    /// Seconds the name prompt waits before saving with the timestamp name (SPEC 8.3: 30 s).
    pub name_prompt_seconds: u32,
    /// Meeting window geometry per display (keyed by display name), in logical pixels.
    pub window: BTreeMap<String, WindowGeometry>,
    /// Ask to transcribe when a meeting app starts using the microphone (SPEC 9.4, off by default).
    pub detect: bool,
    /// Apps that count as meetings, matched against part of the app's name or binary (or bundle
    /// id): for meeting detection, and on macOS for `only_meeting_apps`.
    pub detect_apps: Vec<String>,
    /// KDE: the meeting window's top-left corner in KWin's global coordinates, read through a
    /// KWin script when the window hides (Wayland clients cannot read or set their position).
    pub kwin_position: Option<(i32, i32)>,
}

/// How the dictation shortcut behaves (SPEC 8.4, 15 q5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trigger {
    /// Tap to start and stop; hold longer than 400 ms for push-to-talk.
    #[default]
    Hybrid,
    /// Tap to start, tap again to stop.
    Toggle,
    /// Push-to-talk only: dictates while the shortcut is held.
    Hold,
}

/// What happens when dictation stops (SPEC 8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasteMode {
    /// Show the text in the pill: Enter pastes, Ctrl+C copies, Esc discards.
    #[default]
    Preview,
    /// Paste at once.
    Direct,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationSettings {
    /// Listen for the dictation shortcut.
    pub enabled: bool,
    pub trigger: Trigger,
    pub paste_mode: PasteMode,
    /// Put back what was on the clipboard after pasting.
    pub restore_clipboard: bool,
    /// Window classes (app ids) that get Ctrl+Shift+V instead of Ctrl+V (Linux).
    pub terminal_classes: Vec<String>,
    /// Dictation language: auto, sv or en.
    pub language: String,
    /// Language of the last dictation; short utterances keep it.
    pub last_lang: tyst_core::transcript::Lang,
    /// macOS shortcuts (Tauri accelerators, matched by key position: `BracketLeft` is Å on a
    /// Swedish keyboard). On Linux the desktop binds the shortcuts (System Settings).
    pub shortcut: String,
    pub meeting_shortcut: String,
    /// Linux: the keyboard portal's restore token is in the keychain ([`crate::secrets`]), so
    /// paste does not ask again.
    pub keyboard_access: bool,
    /// Where versions before 0.2 kept that token. Read once to move it into the keychain
    /// (issue #8), never written back.
    #[serde(skip_serializing)]
    pub keyboard_token: Option<String>,
}

pub const DEFAULT_TERMINALS: &[&str] =
    &["com.mitchellh.ghostty", "org.kde.konsole", "kitty", "alacritty", "foot", "wezterm"];

impl Default for DictationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: Trigger::Hybrid,
            paste_mode: PasteMode::Preview,
            restore_clipboard: true,
            terminal_classes: DEFAULT_TERMINALS.iter().map(|s| s.to_string()).collect(),
            language: "auto".into(),
            last_lang: tyst_core::transcript::Lang::Sv,
            shortcut: "Super+BracketLeft".into(),
            meeting_shortcut: "Super+Shift+BracketLeft".into(),
            keyboard_access: false,
            keyboard_token: None,
        }
    }
}

/// Whether a window class belongs to a terminal in `list`: equal ignoring case, or the last part
/// of a reverse-DNS app id (`wezterm` matches `org.wezfurlong.wezterm`).
pub fn is_terminal(class: &str, list: &[String]) -> bool {
    let class = class.trim().to_ascii_lowercase();
    if class.is_empty() {
        return false;
    }
    list.iter().map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()).any(|t| {
        class == t || class.rsplit('.').next() == Some(t.as_str()) || t.rsplit('.').next() == Some(class.as_str())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            onboarded: false,
            transcripts_dir: None,
            models_dir: None,
            language: "auto".into(),
            labels: SpeakerLabels::default(),
            launch_at_login: true,
            threads: 4,
            models_idle_minutes: 0,
            meetings: MeetingSettings::default(),
            dictation: DictationSettings::default(),
            updates: UpdateSettings::default(),
        }
    }
}

impl Default for MeetingSettings {
    fn default() -> Self {
        Self {
            system_audio: true,
            only_meeting_apps: true,
            echo_cancellation: true,
            show_window_on_start: true,
            compact: false,
            name_prompt_seconds: 30,
            window: BTreeMap::new(),
            detect: false,
            detect_apps: tyst_platform::mic_watch::DEFAULT_APPS.iter().map(|s| s.to_string()).collect(),
            kwin_position: None,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TYST_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Tyst")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("tyst")
    }
}

/// Suggested transcripts folder for onboarding: `~/Documents/Tyst` (or `~/Tyst`).
pub fn default_transcripts_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let docs = home.join("Documents");
    if docs.is_dir() { docs.join("Tyst") } else { home.join("Tyst") }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Loads the config, or the defaults when there is none. A broken file is kept aside as
    /// `config.toml.broken` rather than silently overwritten.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("config {} unreadable ({e}), using defaults", path.display());
                    let _ = std::fs::rename(&path, path.with_extension("toml.broken"));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        write_toml(&Self::path(), self)
    }

    pub fn models_dir(&self) -> PathBuf {
        self.models_dir.clone().unwrap_or_else(tyst_core::models::default_models_dir)
    }
}

pub fn vocabulary_path() -> PathBuf {
    config_dir().join("vocabulary.toml")
}

pub fn load_vocabulary() -> VocabularyFile {
    let path = vocabulary_path();
    if !path.exists() {
        return VocabularyFile::default();
    }
    tyst_core::vocabulary::load_file(&path).unwrap_or_else(|e| {
        log::error!("vocabulary unreadable: {e}");
        VocabularyFile::default()
    })
}

pub fn save_vocabulary(v: &VocabularyFile) -> Result<(), String> {
    write_toml(&vocabulary_path(), v)
}

/// Writes atomically (temp file + rename) so a crash never leaves half a file; the file is
/// `0600` in a `0700` folder.
pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = toml::to_string_pretty(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        tyst_core::private_fs::create_own_dir(dir).map_err(|e| e.to_string())?;
    }
    tyst_core::private_fs::write_replace(path, text.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let c: Config = toml::from_str("onboarded = true\n[meetings]\nsystem_audio = false\n").unwrap();
        assert!(c.onboarded);
        assert!(!c.meetings.system_audio);
        assert_eq!(c.meetings.name_prompt_seconds, 30);
        assert!(c.launch_at_login);
        assert_eq!(c.labels.me, "Me");
    }

    #[test]
    fn old_files_get_dictation_defaults() {
        let c: Config = toml::from_str("onboarded = true\n").unwrap();
        assert!(c.dictation.enabled);
        assert_eq!(c.dictation.trigger, Trigger::Hybrid);
        assert_eq!(c.dictation.paste_mode, PasteMode::Preview);
        assert!(c.dictation.restore_clipboard);
        let c: Config = toml::from_str("[dictation]\ntrigger = \"hold\"\npaste_mode = \"direct\"\n").unwrap();
        assert_eq!(c.dictation.trigger, Trigger::Hold);
        assert_eq!(c.dictation.paste_mode, PasteMode::Direct);
    }

    #[test]
    fn spots_terminals() {
        let list: Vec<String> = DEFAULT_TERMINALS.iter().map(|s| s.to_string()).collect();
        for class in
            ["org.kde.konsole", "com.mitchellh.ghostty", "kitty", "Alacritty", "foot", "org.wezfurlong.wezterm"]
        {
            assert!(is_terminal(class, &list), "{class}");
        }
        for class in ["org.kde.kate", "firefox", "Slack", "code", ""] {
            assert!(!is_terminal(class, &list), "{class}");
        }
        // A bare class matches a reverse-DNS entry too (X11 WM_CLASS "konsole").
        assert!(is_terminal("konsole", &list));
    }

    #[test]
    fn keyboard_token_is_read_but_never_written() {
        let c: Config = toml::from_str("[dictation]\nkeyboard_token = \"secret\"\n").unwrap();
        assert_eq!(c.dictation.keyboard_token.as_deref(), Some("secret"));
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(!text.contains("secret"), "{text}");
        assert!(!serde_json::to_string(&c).unwrap().contains("secret"));
    }

    #[test]
    fn round_trips() {
        let mut c = Config { transcripts_dir: Some("/tmp/x".into()), ..Default::default() };
        c.meetings.window.insert("DP-1".into(), WindowGeometry { x: 1.0, y: 2.0, width: 420.0, height: 260.0 });
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
    }
}
