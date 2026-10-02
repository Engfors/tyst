//! Speech recognition engines (SPEC 4.4).

pub mod onnx;
pub mod tdt;
pub mod tokens;

use crate::Result;

/// Recognizes one segment of 16 kHz mono audio.
pub trait AsrEngine: Send {
    /// Engine id with model revision, e.g. `pianissimo-sv-int8@63730c6`.
    fn id(&self) -> &str;
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<AsrResult>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AsrResult {
    pub text: String,
    pub tokens: Vec<Token>,
    /// Mean token log-probability; 0 is best, `-inf` without tokens.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub text: String,
    /// Seconds from the segment start.
    pub start: f32,
    pub logprob: f32,
}
