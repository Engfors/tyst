//! `tyst-cli live`: live transcription from the microphone, or from a file played back in real
//! time (`--simulate`), with the Phase 1 measurements: partial latency, end-of-speech-to-final
//! latency, CPU use and memory.

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use chrono::Local;
use clap::Args;
use tyst_core::journal::Journal;
use tyst_core::markdown;
use tyst_core::pipeline::PipelineEvent;
use tyst_core::resample::{Resampler, SAMPLE_RATE};
use tyst_core::transcript::{Channel, Session, SessionInfo, SpeakerLabels, new_session_id};
use tyst_platform::AudioChunk;

use crate::stats::{self, summarize};
use crate::transcribe::app_name;
use crate::{EngineArgs, setup};

#[derive(Args)]
pub struct LiveArgs {
    /// Capture the default microphone.
    #[arg(long, conflicts_with = "simulate")]
    pub mic: bool,
    /// Play this audio file into the pipeline in real time instead of using a microphone.
    #[arg(long, value_name = "FILE")]
    pub simulate: Option<PathBuf>,
    /// Playback speed for --simulate (1.0 = real time).
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,
    /// Stop after this many seconds (default: Enter, or the end of the --simulate file).
    #[arg(long)]
    pub seconds: Option<f64>,
    /// Save a Markdown transcript into this folder when stopping (journaled while running).
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
    #[arg(long)]
    pub title: Option<String>,
    /// Do not print transcript text, only the measurements.
    #[arg(long)]
    pub quiet: bool,
    /// Write the measurements as JSON to this file.
    #[arg(long)]
    pub stats_json: Option<PathBuf>,
    #[command(flatten)]
    pub engine: EngineArgs,
}

/// Maps stream positions (16 kHz samples) to the wall-clock time they were captured.
struct CaptureClock {
    marks: VecDeque<(u64, Instant)>,
}

impl CaptureClock {
    fn record(&mut self, end_sample: u64, at: Instant) {
        self.marks.push_back((end_sample, at));
        while self.marks.len() > 20_000 {
            self.marks.pop_front();
        }
    }

    /// Capture time of the chunk containing `sample`.
    fn time_of(&self, sample: u64) -> Option<Instant> {
        self.marks.iter().find(|(end, _)| *end >= sample).map(|(_, t)| *t)
    }
}

#[derive(serde::Serialize)]
struct LiveStats {
    source: String,
    threads: usize,
    audio_seconds: f64,
    wall_seconds: f64,
    model_load_seconds: f64,
    cpu_seconds: f64,
    /// CPU time / wall time, in % of one core (SPEC 6.2 target: 30 % on average in a meeting).
    cpu_percent_of_one_core: f64,
    speech_seconds: f64,
    partials: usize,
    finals: usize,
    /// Capture of the newest audio in a partial -> partial shown (target ≤ 1.5 s).
    partial_latency: stats::Summary,
    /// End of speech -> final shown, including the 0.5 s end-of-speech silence (target ≤ 1.0 s).
    /// Only segments ended by silence: a force-cut segment ends at its cut point, up to 2 s before
    /// the 15 s mark that triggers it, so it has no end of speech to measure from.
    final_latency: stats::Summary,
    /// Finals ended by the 15 s force-cut (continuous speech), left out of `final_latency`.
    forced_finals: usize,
    /// Final decode time alone.
    final_decode: stats::Summary,
    decode_seconds_total: f64,
    peak_rss_mb: f64,
}

pub fn run(args: LiveArgs) -> Result<()> {
    if !args.mic && args.simulate.is_none() {
        bail!("pass --mic or --simulate <file>");
    }
    let rt = setup::load(&args.engine)?;
    let mut pipeline = rt.pipeline(Channel::Me, true)?;
    let stop = Arc::new(AtomicBool::new(false));

    let (tx, rx) = mpsc::channel::<AudioChunk>();
    #[cfg(feature = "mic")]
    let mut mic = None;
    let source_name;
    if let Some(path) = &args.simulate {
        let audio = tyst_core::audio_file::decode_mono(path)?;
        source_name = format!("simulate:{} (x{})", path.file_name().unwrap_or_default().to_string_lossy(), args.speed);
        spawn_playback(audio, args.speed, tx, stop.clone());
    } else {
        #[cfg(feature = "mic")]
        {
            use tyst_platform::AudioSource;
            let mut m = tyst_platform::mic::MicSource::new();
            m.start(tx)?;
            source_name = format!("mic:{} @ {} Hz", m.device_name(), m.sample_rate());
            mic = Some(m);
        }
        #[cfg(not(feature = "mic"))]
        {
            drop(tx);
            bail!("built without microphone support (feature `mic`)");
        }
    }
    if args.seconds.is_none() && args.simulate.is_none() {
        let stop = stop.clone();
        eprintln!("listening… press Enter to stop");
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            stop.store(true, Ordering::SeqCst);
        });
    }

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

    let tty = std::io::stderr().is_terminal() && !args.quiet;
    let t_start = Instant::now();
    let cpu_start = stats::cpu_time();
    let mut clock = CaptureClock { marks: VecDeque::new() };
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut position: u64 = 0;
    let (mut partial_lat, mut final_lat, mut final_dec) = (Vec::new(), Vec::new(), Vec::new());
    let (mut partials, mut forced_finals, mut decode_total, mut speech) = (0usize, 0usize, 0.0f64, 0.0f64);
    let mut segments = Vec::new();

    let mut process = |events: Vec<PipelineEvent>, clock: &CaptureClock| -> Result<()> {
        let now = Instant::now();
        for e in events {
            match e {
                PipelineEvent::Partial { text, stats, .. } => {
                    partials += 1;
                    decode_total += stats.elapsed.as_secs_f64();
                    if let Some(t) = clock.time_of(stats.audio_end_sample) {
                        partial_lat.push(now.duration_since(t).as_secs_f64());
                    }
                    if tty {
                        let shown: String =
                            text.chars().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect();
                        eprint!("\r\x1b[2K… {shown}");
                        let _ = std::io::stderr().flush();
                    }
                }
                PipelineEvent::Final { segment, stats, forced } => {
                    decode_total += stats.elapsed.as_secs_f64();
                    final_dec.push(stats.elapsed.as_secs_f64());
                    speech += (segment.end - segment.start).as_secs_f64();
                    if forced {
                        forced_finals += 1;
                    } else if let Some(t) = clock.time_of(stats.audio_end_sample) {
                        final_lat.push(now.duration_since(t).as_secs_f64());
                    }
                    if tty {
                        eprint!("\r\x1b[2K");
                    }
                    if !args.quiet {
                        println!("[{:>7.1}s] {}", segment.start.as_secs_f32(), segment.text);
                    }
                    if let Some(j) = journal.as_mut() {
                        j.append(&segment)?;
                    }
                    segments.push(segment);
                }
                PipelineEvent::Dropped { .. } => {
                    if tty {
                        eprint!("\r\x1b[2K");
                    }
                }
            }
        }
        Ok(())
    };

    let deadline = args.seconds.map(|s| t_start + Duration::from_secs_f64(s));
    loop {
        if stop.load(Ordering::SeqCst) || deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let chunk = match recv(&rx) {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(()) => break, // source finished
        };
        let r = match &mut resampler {
            Some((rate, r)) if *rate == chunk.sample_rate => r,
            _ => {
                resampler = Some((chunk.sample_rate, Resampler::new(chunk.sample_rate, SAMPLE_RATE)));
                &mut resampler.as_mut().unwrap().1
            }
        };
        let pcm = r.push(&chunk.samples);
        position += pcm.len() as u64;
        clock.record(position, chunk.captured_at);
        let events = pipeline.push(&pcm)?;
        process(events, &clock)?;
    }
    #[cfg(feature = "mic")]
    if let Some(mut m) = mic.take() {
        use tyst_platform::AudioSource;
        m.stop()?;
    }
    stop.store(true, Ordering::SeqCst);
    let events = pipeline.flush()?;
    process(events, &clock)?;

    let wall = t_start.elapsed().as_secs_f64();
    let cpu = (stats::cpu_time() - cpu_start).as_secs_f64();
    let s = LiveStats {
        source: source_name,
        threads: args.engine.threads,
        audio_seconds: position as f64 / SAMPLE_RATE as f64,
        wall_seconds: wall,
        model_load_seconds: rt.load_time.as_secs_f64(),
        cpu_seconds: cpu,
        cpu_percent_of_one_core: 100.0 * cpu / wall.max(1e-9),
        speech_seconds: speech,
        partials,
        finals: segments.len(),
        partial_latency: summarize(&partial_lat),
        final_latency: summarize(&final_lat),
        forced_finals,
        final_decode: summarize(&final_dec),
        decode_seconds_total: decode_total,
        peak_rss_mb: stats::peak_rss_mb(),
    };
    eprintln!();
    eprintln!("source            {}", s.source);
    eprintln!(
        "audio             {:.1} s ({:.1} s speech in {} segments, {} partials)",
        s.audio_seconds, s.speech_seconds, s.finals, s.partials
    );
    eprintln!("partial latency   {}   (target ≤ 1.5 s)", s.partial_latency);
    eprintln!(
        "final latency     {}   (end of speech → final, incl. 0.5 s silence; target ≤ 1.0 s; {} force-cut finals not counted)",
        s.final_latency, s.forced_finals
    );
    eprintln!("final decode      {}", s.final_decode);
    eprintln!(
        "CPU               {:.1} % of one core on average ({:.1} s CPU over {:.1} s, {} threads; target ≤ 30 %)",
        s.cpu_percent_of_one_core, s.cpu_seconds, s.wall_seconds, s.threads
    );
    eprintln!(
        "decode time       {:.1} s total (RTF {:.3} over speech)",
        s.decode_seconds_total,
        s.decode_seconds_total / s.speech_seconds.max(1e-9)
    );
    eprintln!("peak RSS          {:.0} MB (model load {:.1} s)", s.peak_rss_mb, s.model_load_seconds);
    if let Some(p) = &args.stats_json {
        std::fs::write(p, serde_json::to_string_pretty(&s)?)?;
    }

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

/// `Ok(None)` on timeout, `Err` when the source is gone.
fn recv(rx: &Receiver<AudioChunk>) -> Result<Option<AudioChunk>, ()> {
    match rx.recv_timeout(Duration::from_millis(100)) {
        Ok(c) => Ok(Some(c)),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(()),
    }
}

/// Sends the file in 20 ms chunks paced by the wall clock, like a capture callback would.
fn spawn_playback(
    audio: tyst_core::audio_file::MonoAudio,
    speed: f64,
    tx: mpsc::Sender<AudioChunk>,
    stop: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let chunk = (audio.sample_rate / 50) as usize;
        let t0 = Instant::now();
        for (i, piece) in audio.samples.chunks(chunk).enumerate() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let due = t0 + Duration::from_secs_f64((i + 1) as f64 * chunk as f64 / audio.sample_rate as f64 / speed);
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            let c = AudioChunk {
                channel: Channel::Me,
                sample_rate: audio.sample_rate,
                samples: piece.to_vec(),
                captured_at: Instant::now(),
            };
            if tx.send(c).is_err() {
                return;
            }
        }
    });
}
