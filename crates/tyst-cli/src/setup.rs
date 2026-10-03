//! Loading models from command-line options (the loading itself lives in `tyst-runtime`).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use tyst_core::router::LanguageMode;
use tyst_core::vocabulary::VocabularyRules;
use tyst_runtime::{EngineOptions, Runtime};

use crate::EngineArgs;

pub fn options(args: &EngineArgs) -> Result<EngineOptions> {
    let mode = LanguageMode::parse(&args.lang).ok_or_else(|| anyhow!("--lang must be auto, sv or en"))?;
    let vocabulary = match &args.vocab {
        Some(p) => VocabularyRules::load(p).with_context(|| format!("loading vocabulary {}", p.display()))?,
        None => VocabularyRules::default(),
    };
    Ok(EngineOptions { models_dir: args.models_dir.clone(), threads: args.threads, mode, vocabulary })
}

pub fn models_dir(args: &EngineArgs) -> PathBuf {
    args.models_dir.clone().unwrap_or_else(tyst_core::models::default_models_dir)
}

/// Loads Pianissimo (or Parakeet when English is forced), registers the lazy English loader,
/// and finds the VAD model.
pub fn load(args: &EngineArgs) -> Result<Runtime> {
    Runtime::load(&options(args)?).context("loading models")
}
