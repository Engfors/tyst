//! Platform layer (SPEC 4.2): OS integrations behind small traits, so the core stays pure.
//!
//! Audio capture: the microphone through cpal (feature `mic`, every platform) and, on Linux,
//! microphone and system audio through PipeWire (feature `pipewire`). Dictation's desktop
//! integration (feature `desktop`): the clipboard, and on Linux the XDG portals and KWin
//! scripting, on macOS app activation and the paste keystroke.

#[cfg(target_os = "linux")]
pub mod appimage;
#[cfg(feature = "desktop")]
pub mod clipboard;
#[cfg(all(target_os = "linux", feature = "desktop"))]
pub mod kwin;
#[cfg(all(target_os = "macos", feature = "desktop"))]
pub mod macos_input;
#[cfg(all(target_os = "macos", feature = "macos-tap"))]
pub mod macos_tap;
#[cfg(feature = "mic")]
pub mod mic;
pub mod mic_watch;
#[cfg(all(target_os = "linux", feature = "pipewire"))]
pub mod pipewire;
#[cfg(all(target_os = "linux", feature = "desktop"))]
pub mod portal;

use std::sync::mpsc::Sender;
use std::time::Instant;

pub use tyst_core::transcript::Channel;

/// A block of captured audio, already downmixed to mono.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub channel: Channel,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
    /// When the last sample of the chunk was captured (approximately: when the callback ran).
    pub captured_at: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no input device available")]
    NoDevice,
    #[error("audio device: {0}")]
    Device(String),
}

/// Desktop integration failures (feature `desktop`).
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("clipboard: {0}")]
    Clipboard(String),
    #[error("desktop portal: {0}")]
    Portal(String),
    #[error("KWin: {0}")]
    KWin(String),
    #[error("keyboard input: {0}")]
    Input(String),
}

/// One capture channel (SPEC 4.4).
pub trait AudioSource {
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
    /// Native sample rate of the stream (valid after `start`).
    fn sample_rate(&self) -> u32;
    /// Human-readable device name, for diagnostics.
    fn device_name(&self) -> String;
    /// Called about twice a second while capturing, on the thread that started the source: the
    /// place to recover from a lost device or follow a change of the default device (SPEC 11).
    fn poll(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}
