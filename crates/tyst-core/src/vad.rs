//! Silero VAD on ONNX Runtime. Both exports are supported: v5 (`input`, `state`, `sr` ->
//! `output`, `stateN`, with 64 samples of context) and v4 (`input`, `sr`, `h`, `c` -> `output`,
//! `hn`, `cn`), including sherpa-onnx's re-export (`x`, `h`, `c` -> `prob`, `new_h`, `new_c`, no
//! `sr`), which is the file Phase 0 used.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::Result;
use crate::asr::onnx::{SessionOptions, build_session};
use crate::segmenter::SpeechDetector;

const WINDOW: usize = 512;
/// Samples of the previous window prepended to each call, as in Silero's own wrapper.
const CONTEXT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SileroVersion {
    V4,
    /// sherpa-onnx's v4 export with renamed tensors and no `sr` input.
    V4Sherpa,
    V5,
}

pub struct SileroVad {
    session: Session,
    version: SileroVersion,
    /// v5: `state` [2, 1, 128]; v4: `h` then `c`, each [2, 1, 64].
    state: Vec<f32>,
    context: Vec<f32>,
}

impl SileroVad {
    pub fn load(path: &Path) -> Result<Self> {
        let session = build_session(path, SessionOptions { threads: 1 })?;
        let has = |name: &str| session.inputs().iter().any(|i| i.name() == name);
        let version = if has("x") && has("h") {
            SileroVersion::V4Sherpa
        } else if has("h") {
            SileroVersion::V4
        } else {
            SileroVersion::V5
        };
        Ok(Self { session, version, state: vec![0.0; 256], context: vec![0.0; CONTEXT] })
    }

    pub fn version(&self) -> SileroVersion {
        self.version
    }

    fn run_v5(&mut self, window: &[f32]) -> Result<f32> {
        let mut input = Vec::with_capacity(CONTEXT + window.len());
        input.extend_from_slice(&self.context);
        input.extend_from_slice(window);
        self.context.copy_from_slice(&window[window.len() - CONTEXT..]);
        let n = input.len();
        let out = self.session.run(ort::inputs![
            "input" => Tensor::from_array(([1usize, n], input))?,
            "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone()))?,
            "sr" => Tensor::from_array(((), vec![16_000i64]))?,
        ])?;
        let (_, prob) = out["output"].try_extract_tensor::<f32>()?;
        let (_, state) = out["stateN"].try_extract_tensor::<f32>()?;
        let p = prob[0];
        self.state.copy_from_slice(state);
        Ok(p)
    }

    fn run_v4(&mut self, window: &[f32]) -> Result<f32> {
        let x = Tensor::from_array(([1usize, window.len()], window.to_vec()))?;
        let h = Tensor::from_array(([2usize, 1, 64], self.state[..128].to_vec()))?;
        let c = Tensor::from_array(([2usize, 1, 64], self.state[128..].to_vec()))?;
        let out = if self.version == SileroVersion::V4Sherpa {
            self.session.run(ort::inputs!["x" => x, "h" => h, "c" => c])?
        } else {
            let sr = Tensor::from_array(((), vec![16_000i64]))?;
            self.session.run(ort::inputs!["input" => x, "sr" => sr, "h" => h, "c" => c])?
        };
        let names =
            if self.version == SileroVersion::V4Sherpa { ["prob", "new_h", "new_c"] } else { ["output", "hn", "cn"] };
        let (_, prob) = out[names[0]].try_extract_tensor::<f32>()?;
        let (_, h) = out[names[1]].try_extract_tensor::<f32>()?;
        let (_, c) = out[names[2]].try_extract_tensor::<f32>()?;
        let p = prob[0];
        self.state[..128].copy_from_slice(h);
        self.state[128..].copy_from_slice(c);
        Ok(p)
    }
}

impl SpeechDetector for SileroVad {
    fn window_size(&self) -> usize {
        WINDOW
    }

    fn prob(&mut self, window: &[f32]) -> Result<f32> {
        match self.version {
            SileroVersion::V5 => self.run_v5(window),
            SileroVersion::V4 | SileroVersion::V4Sherpa => self.run_v4(window),
        }
    }

    fn reset(&mut self) {
        self.state.iter_mut().for_each(|x| *x = 0.0);
        self.context.iter_mut().for_each(|x| *x = 0.0);
    }
}
