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
    pub meetings: MeetingSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeetingSettings {
    /// Capture system audio as Others (off: Me only).
    pub system_audio: bool,
    /// Show the meeting window when recording starts.
    pub show_window_on_start: bool,
    /// Start in the compact one-line mode.
    pub compact: bool,
    /// Seconds the name prompt waits before saving with the timestamp name (SPEC 8.3: 30 s).
    pub name_prompt_seconds: u32,
    /// Meeting window geometry per display (keyed by display name), in logical pixels.
    pub window: BTreeMap<String, WindowGeometry>,
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
            meetings: MeetingSettings::default(),
        }
    }
}

impl Default for MeetingSettings {
    fn default() -> Self {
        Self {
            system_audio: true,
            show_window_on_start: true,
            compact: false,
            name_prompt_seconds: 30,
            window: BTreeMap::new(),
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

/// Writes atomically (temp file + rename) so a crash never leaves half a file.
pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = toml::to_string_pretty(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
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
    fn round_trips() {
        let mut c = Config { transcripts_dir: Some("/tmp/x".into()), ..Default::default() };
        c.meetings.window.insert("DP-1".into(), WindowGeometry { x: 1.0, y: 2.0, width: 420.0, height: 260.0 });
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
    }
}
