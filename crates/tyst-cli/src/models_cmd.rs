//! `tyst-cli models`: list, verify and download the pinned models (SPEC 5.2).
//!
//! Downloading (`tyst_runtime::fetch`) is one of the two network uses the SPEC allows; it only
//! happens on an explicit `fetch`.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use tyst_core::models::{self, FileStatus, Manifest};
use tyst_runtime::fetch::{DEFAULT_MODELS, Progress};

use crate::{EngineArgs, setup};

#[derive(Args)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub action: ModelsAction,
    #[command(flatten)]
    pub engine: EngineArgs,
}

#[derive(Subcommand)]
pub enum ModelsAction {
    /// Show the pinned models and whether they are installed.
    List,
    /// Check installed files: size plus a SHA-256 stamp (files never hashed are hashed once);
    /// with --full every file is hashed again.
    Verify {
        #[arg(long)]
        full: bool,
    },
    /// Download models (default: Pianissimo and Silero VAD; add parakeet-v3-int8 for forced English).
    Fetch { ids: Vec<String> },
}

pub fn run(args: ModelsArgs) -> Result<()> {
    let manifest = Manifest::builtin();
    let dir = setup::models_dir(&args.engine);
    match args.action {
        ModelsAction::List => {
            println!("models directory: {}", dir.display());
            for (id, spec) in &manifest.models {
                let installed = models::installed_dir(spec, &dir).is_ok();
                println!(
                    "{:<22} {:>7.0} MB  {:<9} {}  {}",
                    id,
                    spec.total_size() as f64 / 1e6,
                    if installed { "installed" } else { "missing" },
                    spec.tagged_id(id),
                    spec.license
                );
            }
        }
        ModelsAction::Verify { full } => {
            let mut bad = 0;
            for (id, spec) in &manifest.models {
                let statuses =
                    models::verify(spec, &dir, if full { models::Check::Full } else { models::Check::Stamp })?;
                // Models outside the default fetch (Parakeet, for forced English) are optional.
                if !DEFAULT_MODELS.contains(&id.as_str()) && statuses.iter().all(|(_, s)| *s == FileStatus::Missing) {
                    println!("{id:<22} not installed (optional: `tyst-cli models fetch {id}`)");
                    continue;
                }
                for (name, status) in statuses {
                    if status != FileStatus::Ok {
                        bad += 1;
                    }
                    println!("{id:<22} {name:<32} {status:?}");
                }
            }
            if bad > 0 {
                bail!("{bad} file(s) missing or wrong");
            }
        }
        ModelsAction::Fetch { ids } => {
            let ids = if ids.is_empty() { DEFAULT_MODELS.iter().map(|s| s.to_string()).collect() } else { ids };
            for id in ids {
                fetch(&id, &dir)?;
            }
        }
    }
    Ok(())
}

fn fetch(id: &str, models_dir: &Path) -> Result<()> {
    let spec = Manifest::builtin().get(id)?.clone();
    eprintln!(
        "{id}: {} from {}@{} ({:.0} MB)",
        spec.dir,
        spec.hf_repo,
        &spec.revision[..7],
        spec.total_size() as f64 / 1e6
    );
    let mut last_report = 0u64;
    let cancel = AtomicBool::new(false);
    tyst_runtime::fetch::fetch(&[id], models_dir, &cancel, &mut |p| match p {
        Progress::Model { .. } => {}
        Progress::Present { file } => eprintln!("  {file} present"),
        Progress::Downloading { file, done, total } => {
            if done == 0 || done - last_report > 50_000_000 {
                eprintln!("  {file} {:.0}%", 100.0 * done as f64 / total as f64);
                last_report = done;
            }
        }
        Progress::Deriving { file } => eprintln!("  {file}: computing from the downloaded encoder"),
        Progress::Verified { file } => eprintln!("  {file} ok (sha256 verified)"),
    })
    .with_context(|| format!("fetching {id}"))
}
