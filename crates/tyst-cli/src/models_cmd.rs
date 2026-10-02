//! `tyst-cli models`: list, verify and download the pinned models (SPEC 5.2).
//!
//! Downloading is one of the two network uses the SPEC allows; it only happens on an explicit
//! `fetch`, from Hugging Face at the pinned revision, and every file is checked against its
//! SHA-256 before it is moved into place.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use tyst_core::models::{self, FileStatus, Manifest, ModelFile, ModelSpec, PIANISSIMO, SILERO_VAD};

use crate::{EngineArgs, setup};

/// What a plain `models fetch` downloads.
const DEFAULT_MODELS: [&str; 2] = [PIANISSIMO, SILERO_VAD];

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
    /// Check installed files (sizes; with --full also SHA-256).
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
                let statuses = models::verify(spec, &dir, full)?;
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
                let spec = manifest.get(&id)?;
                fetch(&id, spec, &dir)?;
            }
        }
    }
    Ok(())
}

fn fetch(id: &str, spec: &ModelSpec, models_dir: &Path) -> Result<()> {
    let dir = models_dir.join(&spec.dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    eprintln!(
        "{id}: {} from {}@{} ({:.0} MB)",
        spec.dir,
        spec.hf_repo,
        &spec.revision[..7],
        spec.total_size() as f64 / 1e6
    );
    let status = models::verify(spec, models_dir, false)?;
    for (file, (_, st)) in spec.files.iter().zip(status) {
        if st == FileStatus::Ok {
            eprintln!("  {} present", file.name);
            continue;
        }
        download(spec, file, &dir)?;
    }
    for file in &spec.derived {
        let path = dir.join(&file.name);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == file.size) {
            eprintln!("  {} present", file.name);
            continue;
        }
        eprintln!("  {}: computing from {} ({:?})", file.name, file.from, file.transform);
        models::derive(spec, models_dir, file)?;
        eprintln!("  {} ok ({:.1} MB, sha256 verified)", file.name, file.size as f64 / 1e6);
    }
    Ok(())
}

/// Downloads to `<name>.part` (resuming a partial download), verifies, then renames.
fn download(spec: &ModelSpec, file: &ModelFile, dir: &Path) -> Result<()> {
    let target = dir.join(&file.name);
    let part = dir.join(format!("{}.part", file.name));
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let agent = agent();
    let url = file.url(spec);
    let mut req = agent.get(&url);
    if have > 0 && have < file.size {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().with_context(|| format!("GET {url}"))?;
    let resumed = resp.status().as_u16() == 206;
    let mut out = if resumed { OpenOptions::new().append(true).open(&part)? } else { File::create(&part)? };
    let mut done = if resumed { have } else { 0 };
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut last_report = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if done - last_report > 50_000_000 {
            eprintln!("  {} {:.0}%", file.name, 100.0 * done as f64 / file.size as f64);
            last_report = done;
        }
    }
    out.sync_all()?;
    drop(out);
    let digest = models::sha256_file(&part)?;
    if digest != file.sha256 {
        std::fs::remove_file(&part)?;
        bail!("{}: checksum mismatch (got {digest}, expected {})", file.name, file.sha256);
    }
    std::fs::rename(&part, &target)?;
    eprintln!("  {} ok ({:.1} MB, sha256 verified)", file.name, file.size as f64 / 1e6);
    Ok(())
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into()
}
