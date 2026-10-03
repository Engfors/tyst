//! `tyst-cli`: the Tyst pipeline without a UI (SPEC 4.3, Phase 1).
//!
//! Transcript text goes to stdout and Markdown files only; logs (stderr, `RUST_LOG`) carry timings,
//! sizes, model names and error codes, never text or audio.

mod bench;
mod live;
mod meeting;
mod models_cmd;
mod setup;
mod stats;
mod transcribe;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tyst-cli", version, about = "Local Swedish/English transcription (headless)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Transcribe an audio file (WAV, FLAC, MP3, Ogg, M4A).
    Transcribe(transcribe::TranscribeArgs),
    /// Live transcription from the microphone (or a file played in real time), with latency and CPU stats.
    Live(live::LiveArgs),
    /// Two-channel meeting (Me + Others) from the mic and system audio, or files; saves Markdown.
    Meeting(meeting::MeetingArgs),
    /// Score a clip manifest (eval/manifest.toml format): WER, term accuracy, speed, memory.
    Bench(bench::BenchArgs),
    /// List, verify or download the pinned models.
    Models(models_cmd::ModelsArgs),
    /// Turn journals of interrupted sessions back into Markdown files.
    Recover(transcribe::RecoverArgs),
}

/// Options shared by every command that runs the pipeline.
#[derive(Args, Clone, Debug)]
pub struct EngineArgs {
    /// Models directory (default: $TYST_MODELS or the platform data dir).
    #[arg(long, global = true)]
    pub models_dir: Option<PathBuf>,
    /// ONNX Runtime intra-op threads for the encoder.
    #[arg(long, default_value_t = 4, global = true)]
    pub threads: usize,
    /// Session language: auto (Pianissimo, labels guessed from text), sv, or en (Parakeet v3).
    #[arg(long, default_value = "auto", global = true)]
    pub lang: String,
    /// Vocabulary file (terms + replacements, TOML like eval/terms.toml).
    #[arg(long, global = true)]
    pub vocab: Option<PathBuf>,
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Transcribe(a) => transcribe::run(a),
        Command::Live(a) => live::run(a),
        Command::Meeting(a) => meeting::run(a),
        Command::Bench(a) => bench::run(a),
        Command::Models(a) => models_cmd::run(a),
        Command::Recover(a) => transcribe::recover(a),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
