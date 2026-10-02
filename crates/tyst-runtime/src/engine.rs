//! Loading the models and building channel pipelines.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tyst_core::asr::AsrEngine;
use tyst_core::asr::onnx::{OnnxModelFiles, OnnxTdtEngine, SessionOptions};
use tyst_core::models::{self, Manifest, PARAKEET, PIANISSIMO, SILERO_VAD};
use tyst_core::pipeline::{ChannelPipeline, EngineSet, PipelineConfig, SharedEngines};
use tyst_core::router::{EngineRole, FixedRouter, LanguageMode};
use tyst_core::transcript::{Channel, Lang};
use tyst_core::vad::SileroVad;
use tyst_core::vocabulary::VocabularyRules;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Models directory (`None`: `$TYST_MODELS` or the platform data dir).
    pub models_dir: Option<PathBuf>,
    /// ONNX Runtime intra-op threads.
    pub threads: usize,
    pub mode: LanguageMode,
    pub vocabulary: VocabularyRules,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self { models_dir: None, threads: 4, mode: LanguageMode::Auto, vocabulary: VocabularyRules::default() }
    }
}

impl EngineOptions {
    pub fn models_dir(&self) -> PathBuf {
        self.models_dir.clone().unwrap_or_else(models::default_models_dir)
    }
}

/// Loaded models, kept warm between sessions.
pub struct Runtime {
    pub engines: SharedEngines,
    pub vad_path: PathBuf,
    pub vocabulary: Arc<VocabularyRules>,
    pub mode: LanguageMode,
    pub load_time: Duration,
}

impl Runtime {
    /// Loads Pianissimo (or Parakeet when English is forced), registers the lazy English loader,
    /// and finds the VAD model.
    pub fn load(opts: &EngineOptions) -> Result<Self> {
        let manifest = Manifest::builtin();
        let dir = opts.models_dir();
        let session = SessionOptions { threads: opts.threads.max(1) };
        let t0 = Instant::now();

        let load_engine = {
            let manifest = manifest.clone();
            let dir = dir.clone();
            move |id: &str| -> tyst_core::Result<Box<dyn AsrEngine>> {
                let spec = manifest.get(id)?;
                let path = models::installed_dir(spec, &dir)?;
                let files = OnnxModelFiles::discover(&path)?;
                Ok(Box::new(OnnxTdtEngine::load(spec.tagged_id(id), &files, session)?))
            }
        };
        let loader = |id: &'static str| {
            let load_engine = load_engine.clone();
            Box::new(move || load_engine(id)) as Box<dyn FnMut() -> tyst_core::Result<Box<dyn AsrEngine>> + Send>
        };
        let mut set = EngineSet::new().with_primary_loader(loader(PIANISSIMO)).with_english_loader(loader(PARAKEET));
        let role = if opts.mode == LanguageMode::English { EngineRole::English } else { EngineRole::Primary };
        set.preload(role)?;
        log::info!("models loaded in {:.2}s", t0.elapsed().as_secs_f32());

        let vad_spec = manifest.get(SILERO_VAD)?;
        let vad_path = models::installed_dir(vad_spec, &dir)?.join(&vad_spec.files[0].name);
        if !vad_path.is_file() {
            return Err(Error::Other(format!("VAD model missing: {}", vad_path.display())));
        }
        Ok(Self {
            engines: set.shared(),
            vad_path,
            vocabulary: Arc::new(opts.vocabulary.clone()),
            mode: opts.mode,
            load_time: t0.elapsed(),
        })
    }

    /// A pipeline for one channel, with its own VAD and router.
    pub fn pipeline(&self, channel: Channel, partials: bool) -> Result<ChannelPipeline<SileroVad>> {
        let vad = SileroVad::load(&self.vad_path)?;
        let initial = if self.mode == LanguageMode::English { Lang::En } else { Lang::Sv };
        let router = FixedRouter::new(self.mode, initial, Duration::from_millis(1500));
        Ok(ChannelPipeline::new(
            channel,
            PipelineConfig { partials, ..Default::default() },
            vad,
            self.engines.clone(),
            Box::new(router),
            self.vocabulary.clone(),
        ))
    }

    /// Engine ids (with revisions) loaded so far.
    pub fn model_ids(&self) -> Vec<String> {
        self.engines.lock().map(|e| e.loaded_ids()).unwrap_or_default()
    }
}
