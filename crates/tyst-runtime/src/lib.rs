//! Runtime glue shared by `tyst-cli` and the app: loading the models into a warm [`Runtime`],
//! downloading them ([`fetch`], one of the two network uses SPEC 0 allows), and running a live
//! two-channel [`meeting`] session or a [`dictation`], and checking for a newer release
//! ([`update`], the other network use).
//!
//! Never log transcript text or audio; timings, sizes, model names and error codes only.

pub mod dictation;
pub mod echo;
pub mod engine;
pub mod fetch;
pub mod meeting;
pub mod playback;
mod queue;
pub mod sources;
pub mod update;

pub use engine::{EngineOptions, Runtime};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] tyst_core::Error),
    #[error(transparent)]
    Capture(#[from] tyst_platform::CaptureError),
    #[error("download: {0}")]
    Download(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
