//! Loading models and building pipelines from command-line options.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tyst_core::asr::AsrEngine;
use tyst_core::asr::onnx::{OnnxModelFiles, OnnxTdtEngine, SessionOptions};
use tyst_core::models::{self, Manifest, PARAKEET, PIANISSIMO, SILERO_VAD};
use tyst_core::pipeline::{ChannelPipeline, EngineSet, PipelineConfig, SharedEngines};
use tyst_core::router::{EngineRole, FixedRouter, LanguageMode};
use tyst_core::transcript::{Channel, Lang};
use tyst_core::vad::SileroVad;
use tyst_core::vocabulary::VocabularyRules;

use crate::EngineArgs;

pub struct Runtime {
    pub engines: SharedEngines,
    pub vad_path: PathBuf,
    pub vocabulary: Arc<VocabularyRules>,
    pub mode: LanguageMode,
    pub load_time: Duration,
}

pub fn models_dir(args: &EngineArgs) -> PathBuf {
    args.models_dir.clone().unwrap_or_else(models::default_models_dir)
}

/// Loads Pianissimo (or Parakeet when English is forced), registers the lazy English loader,
/// and finds the VAD model.
pub fn load(args: &EngineArgs) -> Result<Runtime> {
    let mode = LanguageMode::parse(&args.lang).ok_or_else(|| anyhow!("--lang must be auto, sv or en"))?;
    let manifest = Manifest::builtin();
    let dir = models_dir(args);
    let opts = SessionOptions { threads: args.threads.max(1) };
    let t0 = Instant::now();

    let load_engine = {
        let manifest = manifest.clone();
        let dir = dir.clone();
        move |id: &str| -> tyst_core::Result<Box<dyn AsrEngine>> {
            let spec = manifest.get(id)?;
            let path = models::installed_dir(spec, &dir)?;
            let files = OnnxModelFiles::discover(&path)?;
            Ok(Box::new(OnnxTdtEngine::load(spec.tagged_id(id), &files, opts)?))
        }
    };
    let loader = |id: &'static str| {
        let load_engine = load_engine.clone();
        Box::new(move || load_engine(id)) as Box<dyn FnMut() -> tyst_core::Result<Box<dyn AsrEngine>> + Send>
    };
    let mut set = EngineSet::new().with_primary_loader(loader(PIANISSIMO)).with_english_loader(loader(PARAKEET));
    let role = if mode == LanguageMode::English { EngineRole::English } else { EngineRole::Primary };
    set.preload(role).context("loading ASR model")?;
    log::info!("models loaded in {:.2}s", t0.elapsed().as_secs_f32());
    let engines = set.shared();

    let vad_spec = manifest.get(SILERO_VAD)?;
    let vad_path = models::installed_dir(vad_spec, &dir)?.join(&vad_spec.files[0].name);
    let vocabulary = match &args.vocab {
        Some(p) => VocabularyRules::load(p).with_context(|| format!("loading vocabulary {}", p.display()))?,
        None => VocabularyRules::default(),
    };
    Ok(Runtime { engines, vad_path, vocabulary: Arc::new(vocabulary), mode, load_time: t0.elapsed() })
}

impl Runtime {
    pub fn pipeline(&self, channel: Channel, partials: bool) -> Result<ChannelPipeline<SileroVad>> {
        let vad = SileroVad::load(&self.vad_path).context("loading Silero VAD")?;
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

    pub fn model_ids(&self) -> Vec<String> {
        self.engines.lock().map(|e| e.loaded_ids()).unwrap_or_default()
    }
}
