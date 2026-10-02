//! NeMo FastConformer-TDT models in onnx-asr layout (Pianissimo, Parakeet v3) on ONNX Runtime.
//!
//! Files: `nemo128.onnx` (audio -> 128-band log-mel features), `encoder-model*.onnx`,
//! `decoder_joint-model*.onnx` (prediction network and joiner fused) and `vocab.txt` (ADR 0001).

use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;

use super::tdt::{JointNetwork, TdtConfig, greedy_decode};
use super::tokens::Vocabulary;
use super::{AsrEngine, AsrResult, Token};
use crate::{Error, Result};

/// Encoder output frame length: 10 ms features, 8x subsampling.
const FRAME_SECONDS: f32 = 0.08;
const MAX_TOKENS_PER_STEP: usize = 10;

#[derive(Debug, Clone)]
pub struct OnnxModelFiles {
    pub preprocessor: PathBuf,
    pub encoder: PathBuf,
    pub decoder_joint: PathBuf,
    pub vocab: PathBuf,
}

impl OnnxModelFiles {
    /// Finds the files in a model directory, preferring int8 weights.
    pub fn discover(dir: &Path) -> Result<Self> {
        let pick = |stem: &str| -> Result<PathBuf> {
            for name in [format!("{stem}.banded.int8.onnx"), format!("{stem}.int8.onnx"), format!("{stem}.onnx")] {
                let p = dir.join(&name);
                if p.is_file() {
                    return Ok(p);
                }
            }
            Err(Error::Model(format!("{}: no {stem}.int8.onnx or {stem}.onnx", dir.display())))
        };
        let need = |name: &str| -> Result<PathBuf> {
            let p = dir.join(name);
            if p.is_file() { Ok(p) } else { Err(Error::Model(format!("{}: missing {name}", dir.display()))) }
        };
        Ok(Self {
            preprocessor: need("nemo128.onnx")?,
            encoder: pick("encoder-model")?,
            decoder_joint: pick("decoder_joint-model")?,
            vocab: need("vocab.txt")?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SessionOptions {
    /// Intra-op threads per session.
    pub threads: usize,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self { threads: 4 }
    }
}

pub(crate) fn build_session(path: &Path, opts: SessionOptions) -> Result<Session> {
    let session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(opts.threads)?
        .with_inter_threads(1)?
        .commit_from_file(path)?;
    Ok(session)
}

pub struct OnnxTdtEngine {
    id: String,
    preprocessor: Session,
    encoder: Session,
    joint: OnnxJoint,
    vocab: Vocabulary,
    tdt: TdtConfig,
}

impl OnnxTdtEngine {
    pub fn load(id: impl Into<String>, files: &OnnxModelFiles, opts: SessionOptions) -> Result<Self> {
        let vocab = Vocabulary::load(&files.vocab)?;
        let tdt = TdtConfig { vocab_size: vocab.len(), blank: vocab.blank(), max_tokens_per_step: MAX_TOKENS_PER_STEP };
        Ok(Self {
            id: id.into(),
            preprocessor: build_session(&files.preprocessor, opts)?,
            encoder: build_session(&files.encoder, opts)?,
            // The decoder runs one tiny step per frame; extra threads only add sync overhead.
            joint: OnnxJoint {
                session: build_session(&files.decoder_joint, SessionOptions { threads: 1 })?,
                state_dim: 640,
            },
            vocab,
            tdt,
        })
    }

    /// Encoder frames for a waveform, as `[frame][feature]`.
    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<Vec<f32>>> {
        let n = pcm.len();
        let waveforms = Tensor::from_array(([1usize, n], pcm.to_vec()))?;
        let lens = Tensor::from_array(([1usize], vec![n as i64]))?;
        let out = self.preprocessor.run(ort::inputs!["waveforms" => waveforms, "waveforms_lens" => lens])?;
        let (fshape, features) = out["features"].try_extract_tensor::<f32>()?;
        let fshape: Vec<usize> = fshape.iter().map(|&d| d as usize).collect();
        let (_, flens) = out["features_lens"].try_extract_tensor::<i64>()?;
        let features = Tensor::from_array((fshape, features.to_vec()))?;
        let flens = Tensor::from_array(([1usize], flens.to_vec()))?;
        drop(out);
        let t_pre = std::time::Instant::now();
        let out = self.encoder.run(ort::inputs!["audio_signal" => features, "length" => flens])?;
        let (eshape, enc) = out["outputs"].try_extract_tensor::<f32>()?;
        let (_, elens) = out["encoded_lengths"].try_extract_tensor::<i64>()?;
        // [1, dim, frames] -> frames x dim
        let (dim, frames) = (eshape[1] as usize, eshape[2] as usize);
        log::trace!("encoder {:.0} ms", t_pre.elapsed().as_secs_f32() * 1000.0);
        let valid = (elens[0].max(0) as usize).min(frames);
        Ok((0..valid).map(|t| (0..dim).map(|d| enc[d * frames + t]).collect()).collect())
    }
}

impl AsrEngine for OnnxTdtEngine {
    fn id(&self) -> &str {
        &self.id
    }

    fn transcribe(&mut self, pcm: &[f32]) -> Result<AsrResult> {
        if pcm.is_empty() {
            return Ok(AsrResult { confidence: f32::NEG_INFINITY, ..Default::default() });
        }
        let t0 = std::time::Instant::now();
        let frames = self.encode(pcm)?;
        let t1 = std::time::Instant::now();
        let decoded = greedy_decode(&mut self.joint, &frames, &self.tdt)?;
        log::debug!(
            "{}: {:.2} s audio, encode {:.0} ms, decode {:.0} ms ({} frames, {} tokens)",
            self.id,
            pcm.len() as f32 / 16_000.0,
            (t1 - t0).as_secs_f32() * 1000.0,
            t1.elapsed().as_secs_f32() * 1000.0,
            frames.len(),
            decoded.tokens.len()
        );
        let tokens = decoded
            .tokens
            .iter()
            .zip(&decoded.frames)
            .zip(&decoded.logprobs)
            .map(|((&id, &frame), &logprob)| Token {
                text: self.vocab.piece(id).to_string(),
                start: frame as f32 * FRAME_SECONDS,
                logprob,
            })
            .collect();
        Ok(AsrResult { text: self.vocab.detokenize(&decoded.tokens), tokens, confidence: decoded.confidence() })
    }
}

struct OnnxJoint {
    session: Session,
    state_dim: usize,
}

/// LSTM states `(input_states_1, input_states_2)`, each `[2, 1, 640]`.
type LstmState = (Vec<f32>, Vec<f32>);

impl JointNetwork for OnnxJoint {
    type State = LstmState;

    fn initial_state(&self) -> LstmState {
        (vec![0.0; 2 * self.state_dim], vec![0.0; 2 * self.state_dim])
    }

    fn step(&mut self, frame: &[f32], prev_token: u32, state: &LstmState) -> Result<(Vec<f32>, LstmState)> {
        let enc = Tensor::from_array(([1usize, frame.len(), 1], frame.to_vec()))?;
        let targets = Tensor::from_array(([1usize, 1], vec![prev_token as i32]))?;
        let target_len = Tensor::from_array(([1usize], vec![1i32]))?;
        let s1 = Tensor::from_array(([2usize, 1, self.state_dim], state.0.clone()))?;
        let s2 = Tensor::from_array(([2usize, 1, self.state_dim], state.1.clone()))?;
        let out = self.session.run(ort::inputs![
            "encoder_outputs" => enc,
            "targets" => targets,
            "target_length" => target_len,
            "input_states_1" => s1,
            "input_states_2" => s2,
        ])?;
        let (_, logits) = out["outputs"].try_extract_tensor::<f32>()?;
        let (_, o1) = out["output_states_1"].try_extract_tensor::<f32>()?;
        let (_, o2) = out["output_states_2"].try_extract_tensor::<f32>()?;
        Ok((logits.to_vec(), (o1.to_vec(), o2.to_vec())))
    }
}
