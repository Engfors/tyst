//! `tyst-cli transcribe` and `tyst-cli recover`.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Local;
use clap::Args;
use tyst_core::journal::{self, Journal};
use tyst_core::markdown;
use tyst_core::pipeline::PipelineEvent;
use tyst_core::transcript::{Channel, Session, SessionInfo, SpeakerLabels, new_session_id};

use crate::{EngineArgs, setup};

#[derive(Args)]
pub struct TranscribeArgs {
    /// Audio file.
    pub file: PathBuf,
    /// Write a Markdown transcript into this folder (journaled while running).
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    /// Meeting title for the Markdown file.
    #[arg(long)]
    pub title: Option<String>,
    /// Treat the file as this speaker channel: me or others.
    #[arg(long, default_value = "others")]
    pub channel: String,
    #[command(flatten)]
    pub engine: EngineArgs,
}

#[derive(Args)]
pub struct RecoverArgs {
    /// Transcripts folder that holds `.tyst-journal/`.
    pub dir: PathBuf,
}

pub fn app_name() -> String {
    format!("Tyst {}", env!("CARGO_PKG_VERSION"))
}

pub fn run(args: TranscribeArgs) -> Result<()> {
    let channel = match args.channel.as_str() {
        "me" => Channel::Me,
        _ => Channel::Others,
    };
    let rt = setup::load(&args.engine)?;
    let t0 = Instant::now();
    let pcm = tyst_core::audio_file::load_16k_mono(&args.file)?;
    let audio_s = pcm.len() as f64 / 16_000.0;
    let mut pipeline = rt.pipeline(channel, false)?;

    let started_at = Local::now().fixed_offset();
    let info = SessionInfo {
        id: new_session_id(&started_at),
        started_at,
        app: app_name(),
        models: rt.model_ids(),
        labels: SpeakerLabels::default(),
    };
    let mut journal = match &args.out_dir {
        Some(dir) => Some(Journal::create(dir, &info)?),
        None => None,
    };

    let mut segments = Vec::new();
    let mut handle = |events: Vec<PipelineEvent>| -> Result<()> {
        for e in events {
            if let PipelineEvent::Final { segment, .. } = e {
                println!("[{:>7.1}s] {}", segment.start.as_secs_f32(), segment.text);
                if let Some(j) = journal.as_mut() {
                    j.append(&segment)?;
                }
                segments.push(segment);
            }
        }
        Ok(())
    };
    for chunk in pcm.chunks(16_000) {
        handle(pipeline.push(chunk)?)?;
    }
    handle(pipeline.flush()?)?;
    let elapsed = t0.elapsed().as_secs_f64();
    eprintln!(
        "{audio_s:.1} s audio, {} segments, {elapsed:.1} s ({:.3} x real time) after {:.1} s model load",
        segments.len(),
        elapsed / audio_s.max(1e-9),
        rt.load_time.as_secs_f64()
    );

    if let (Some(dir), Some(journal)) = (&args.out_dir, journal) {
        let mut info = info;
        info.models = rt.model_ids();
        let session =
            Session { info, ended_at: Local::now().fixed_offset(), title: args.title, segments, markers: vec![] };
        let path = markdown::save(&session, dir)?;
        journal.remove()?;
        eprintln!("saved {}", path.display());
    }
    Ok(())
}

pub fn recover(args: RecoverArgs) -> Result<()> {
    let orphans = journal::find_orphans(&args.dir)?;
    if orphans.is_empty() {
        eprintln!("no unsaved sessions in {}", args.dir.display());
    }
    for path in orphans {
        let session = journal::recover(&path).with_context(|| format!("reading {}", path.display()))?;
        let out = markdown::save(&session, &args.dir)?;
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        eprintln!("recovered session from {} -> {}", session.info.started_at.format("%Y-%m-%d %H:%M"), out.display());
    }
    Ok(())
}
