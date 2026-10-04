//! `tyst-cli dictate`: the app's dictation session without the pill (the same
//! `tyst_runtime::dictation`), measuring the Phase 3 target: stop -> final text ≤ 1 s.
//!
//! With `--mic`, Enter starts and stops each dictation; with `--simulate`, each file is played in
//! real time and the dictation stops the moment it ends, like releasing the key after the last
//! word.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;
use tyst_core::transcript::{Channel, Lang};
use tyst_runtime::Runtime;
use tyst_runtime::dictation::{Dictated, Dictation, DictationEvent, DictationOptions};
use tyst_runtime::meeting::SourceFactory;
use tyst_runtime::playback::FilePlayback;

use crate::stats::{self, summarize};
use crate::{EngineArgs, meeting, setup};

#[derive(Args)]
pub struct DictateArgs {
    /// Dictate from the default microphone: Enter starts and stops, an empty line after a
    /// result starts the next one, `q` quits.
    #[arg(long, conflicts_with = "simulate")]
    pub mic: bool,
    /// Play these files as dictations, one after the other, stopping as each ends.
    #[arg(long, value_name = "FILE", num_args = 1..)]
    pub simulate: Vec<PathBuf>,
    /// Playback speed for --simulate (1.0 = real time).
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,
    /// For --simulate: cut each file this many seconds after its last loud 20 ms frame, like
    /// a user who stops right after the last word (default: play the whole file).
    #[arg(long, value_name = "SECONDS")]
    pub stop_after_speech: Option<f64>,
    /// Do not print the dictated text, only the measurements.
    #[arg(long)]
    pub quiet: bool,
    /// Write the measurements as JSON to this file.
    #[arg(long)]
    pub stats_json: Option<PathBuf>,
    #[command(flatten)]
    pub engine: EngineArgs,
}

#[derive(serde::Serialize)]
struct DictateStats {
    threads: usize,
    dictations: usize,
    audio_seconds: f64,
    /// Stop -> final text ready (target ≤ 1.0 s).
    stop_to_text: stats::Summary,
    /// Decode time of the open tail after stop.
    tail_decode: stats::Summary,
    /// First partial text after the start, from the start.
    first_text: stats::Summary,
    peak_rss_mb: f64,
}

pub fn run(args: DictateArgs) -> Result<()> {
    if !args.mic && args.simulate.is_empty() {
        bail!("pass --mic or --simulate <file>…");
    }
    let rt = setup::load(&args.engine)?;
    let mut lang = if rt.mode == tyst_core::router::LanguageMode::English { Lang::En } else { Lang::Sv };
    let mut results: Vec<(Dictated, Option<Duration>)> = Vec::new();
    let tty = std::io::stderr().is_terminal() && !args.quiet;

    if args.mic {
        let lines = stdin_lines();
        loop {
            eprintln!("press Enter to start dictating (q to quit)");
            match lines.recv() {
                Ok(l) if l.trim() == "q" => break,
                Ok(_) => {}
                Err(_) => break,
            }
            let (d, rx, t0) = start(&rt, meeting::mic_source()?, lang)?;
            eprintln!("listening on {}… press Enter to stop", d.device_name());
            let first = watch(&rx, t0, tty, || lines.try_recv().is_ok());
            let out = d.finish()?;
            lang = out.lang;
            report(&out, args.quiet);
            results.push((out, first));
        }
    } else {
        for path in &args.simulate {
            let mut audio = tyst_core::audio_file::decode_mono(path)?;
            if let Some(tail) = args.stop_after_speech {
                trim_after_speech(&mut audio, tail);
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let src = FilePlayback::from_audio(audio, Channel::Me, args.speed, name);
            let done = src.finished_flag();
            let factory: SourceFactory = Box::new(move || Ok(Box::new(src) as _));
            let (d, rx, t0) = start(&rt, factory, lang)?;
            let first = watch(&rx, t0, tty, || done.load(std::sync::atomic::Ordering::SeqCst));
            let out = d.finish()?;
            lang = out.lang;
            if !args.quiet {
                eprint!("{}: ", path.file_name().unwrap_or_default().to_string_lossy());
            }
            report(&out, args.quiet);
            results.push((out, first));
        }
    }

    if results.is_empty() {
        return Ok(());
    }
    let s = DictateStats {
        threads: args.engine.threads,
        dictations: results.len(),
        audio_seconds: results.iter().map(|(r, _)| r.duration.as_secs_f64()).sum(),
        stop_to_text: summarize(&results.iter().map(|(r, _)| r.finish_latency.as_secs_f64()).collect::<Vec<_>>()),
        tail_decode: summarize(&results.iter().map(|(r, _)| r.tail_decode.as_secs_f64()).collect::<Vec<_>>()),
        first_text: summarize(&results.iter().filter_map(|(_, f)| f.map(|d| d.as_secs_f64())).collect::<Vec<_>>()),
        peak_rss_mb: stats::peak_rss_mb(),
    };
    eprintln!();
    eprintln!("dictations        {} ({:.1} s audio, {} threads)", s.dictations, s.audio_seconds, s.threads);
    eprintln!("stop → text       {}   (target ≤ 1.0 s)", s.stop_to_text);
    eprintln!("tail decode       {}", s.tail_decode);
    eprintln!("first live text   {}   (after the start)", s.first_text);
    eprintln!("peak RSS          {:.0} MB", s.peak_rss_mb);
    if let Some(p) = &args.stats_json {
        std::fs::write(p, serde_json::to_string_pretty(&s)?)?;
    }
    Ok(())
}

fn start(rt: &Runtime, source: SourceFactory, lang: Lang) -> Result<(Dictation, Receiver<DictationEvent>, Instant)> {
    let pipeline = rt.pipeline_with(Channel::Me, true, rt.mode, lang)?;
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    let d = Dictation::start(pipeline, source, DictationOptions { mode: rt.mode, initial: lang }, tx)?;
    Ok((d, rx, t0))
}

/// Shows live text until `stop()` says so; returns when the first text appeared.
fn watch(rx: &Receiver<DictationEvent>, t0: Instant, tty: bool, mut stop: impl FnMut() -> bool) -> Option<Duration> {
    let mut first = None;
    while !stop() {
        let Ok(e) = rx.recv_timeout(Duration::from_millis(20)) else { continue };
        match e {
            DictationEvent::Text { committed, partial } => {
                first.get_or_insert_with(|| t0.elapsed());
                if tty {
                    let line = format!("{committed} {partial}");
                    let shown: String = line.chars().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect();
                    eprint!("\r\x1b[2K… {}", shown.trim());
                    let _ = std::io::stderr().flush();
                }
            }
            DictationEvent::CaptureError { message } => eprintln!("capture: {message}"),
            DictationEvent::Error { message } => eprintln!("error: {message}"),
            DictationEvent::Level { .. } => {}
        }
    }
    if tty {
        eprint!("\r\x1b[2K");
    }
    first
}

fn report(out: &Dictated, quiet: bool) {
    if !quiet {
        println!("{}", out.text);
    }
    eprintln!(
        "  [{} · {:.1} s audio · stop → text {} ms (tail decode {} ms)]",
        out.lang.code(),
        out.duration.as_secs_f32(),
        out.finish_latency.as_millis(),
        out.tail_decode.as_millis()
    );
}

/// Cuts `audio` `tail` seconds after the last 20 ms frame louder than a tenth of the loudest.
fn trim_after_speech(audio: &mut tyst_core::audio_file::MonoAudio, tail: f64) {
    let frame = (audio.sample_rate / 50) as usize;
    let rms: Vec<f32> =
        audio.samples.chunks(frame).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let loudest = rms.iter().copied().fold(0.0f32, f32::max);
    if let Some(last) = rms.iter().rposition(|&r| r > loudest * 0.1) {
        let end = (last + 1) * frame + (tail * audio.sample_rate as f64) as usize;
        audio.samples.truncate(end);
    }
}

fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(l) = line else { return };
            if tx.send(l).is_err() {
                return;
            }
        }
    });
    rx
}
