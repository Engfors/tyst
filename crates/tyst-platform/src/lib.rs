//! Platform layer (SPEC 4.2): OS integrations behind small traits, so the core stays pure.
//!
//! Audio capture: the microphone through cpal (feature `mic`, every platform) and, on Linux,
//! microphone and system audio through PipeWire (feature `pipewire`). Hotkeys, paste and window
//! behavior arrive with later phases.

#[cfg(feature = "mic")]
pub mod mic;
#[cfg(all(target_os = "linux", feature = "pipewire"))]
pub mod pipewire;

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
