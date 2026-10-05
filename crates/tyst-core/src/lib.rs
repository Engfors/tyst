//! Tyst core: the transcription pipeline from 16 kHz audio to Markdown.
//!
//! Pure Rust plus ONNX Runtime. No network access, no UI and no OS APIs (SPEC 0, 4.2).
//! Never log transcript text or audio; timings, sizes, model names and error codes only.

pub mod asr;
pub mod audio_file;
pub mod encoder_rewrite;
pub mod journal;
pub mod markdown;
pub mod models;
pub mod pipeline;
pub mod private_fs;
pub mod resample;
pub mod router;
pub mod segmenter;
pub mod text;
pub mod transcript;
pub mod vad;
pub mod vocabulary;

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("onnx runtime: {0}")]
    Ort(String),
    #[error("model: {0}")]
    Model(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("config: {0}")]
    Config(String),
    #[error("journal: {0}")]
    Journal(String),
}

impl Error {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io { path: path.as_ref().to_path_buf(), source }
    }
}

impl<T> From<ort::Error<T>> for Error {
    fn from(e: ort::Error<T>) -> Self {
        Error::Ort(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
