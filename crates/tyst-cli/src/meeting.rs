//! `tyst-cli meeting`: a two-channel meeting session without the app (the same
//! `tyst_runtime::meeting` the app runs). Me and Others come from the microphone and system
//! audio, or from files played in real time, and the result is saved as Markdown.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;
use tyst_core::transcript::{Channel, SpeakerLabels};
use tyst_runtime::meeting::{Meeting, MeetingEvent, MeetingOptions, SourceFactory};
use tyst_runtime::playback::FilePlayback;

use crate::transcribe::app_name;
use crate::{EngineArgs, setup};

#[derive(Args)]
pub struct MeetingArgs {
    /// Transcripts folder (journal while running, Markdown when stopped).
    #[arg(long)]
    pub out_dir: PathBuf,
    /// Me from the default microphone.
    #[arg(long, conflicts_with = "me")]
    pub mic: bool,
    /// Others from system audio (Linux: monitor of the default PipeWire sink; macOS: process tap).
    #[arg(long, conflicts_with = "others")]
    pub system: bool,
    /// Me from this file, played in real time.
    #[arg(long, value_name = "FILE")]
    pub me: Option<PathBuf>,
    /// Others from this file, played in real time.
    #[arg(long, value_name = "FILE")]
    pub others: Option<PathBuf>,
    /// Stop after this many seconds (default: Enter, or when the files have played).
    #[arg(long)]
    pub seconds: Option<f64>,
    #[arg(long)]
    pub title: Option<String>,
    /// Do not print transcript text.
    #[arg(long)]
    pub quiet: bool,
    #[command(flatten)]
    pub engine: EngineArgs,
}

pub fn run(args: MeetingArgs) -> Result<()> {
    let mut sources: Vec<(Channel, SourceFactory)> = Vec::new();
    let mut finished: Vec<Arc<AtomicBool>> = Vec::new();
    for (channel, file) in [(Channel::Me, &args.me), (Channel::Others, &args.others)] {
        if let Some(path) = file {
            let src = FilePlayback::open(path, channel, 1.0)?;
            finished.push(src.finished_flag());
            sources.push((channel, Box::new(move || Ok(Box::new(src) as _))));
        }
    }
    if args.mic {
        sources.push((Channel::Me, mic_source()?));
    }
    if args.system {
        sources.push((Channel::Others, system_source()?));
    }
    if sources.is_empty() {
        bail!("pass --mic/--me and/or --system/--others");
    }

    let rt = setup::load(&args.engine)?;
    let (tx, rx) = mpsc::channel();
    let opts = MeetingOptions {
        transcripts_dir: args.out_dir.clone(),
        labels: SpeakerLabels::default(),
        app: app_name(),
        mode: rt.mode,
        sources,
    };
    let meeting = Meeting::start(&rt, opts, tx)?;
    let stop = Arc::new(AtomicBool::new(false));
    if args.seconds.is_none() && finished.is_empty() {
        let stop = stop.clone();
        eprintln!("recording… press Enter to stop");
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            stop.store(true, Ordering::SeqCst);
        });
    }
    let deadline = args.seconds.map(|s| Instant::now() + Duration::from_secs_f64(s));
    let print = |e: MeetingEvent| match e {
        MeetingEvent::Final(s) if !args.quiet => {
            println!("[{:>7.1}s] {:<6} {}", s.start.as_secs_f32(), format!("{:?}", s.channel), s.text)
        }
        MeetingEvent::CaptureError { channel, message } => eprintln!("{channel:?} capture: {message}"),
        MeetingEvent::Error { message, .. } => eprintln!("error: {message}"),
        _ => {}
    };
    loop {
        if stop.load(Ordering::SeqCst)
            || deadline.is_some_and(|d| Instant::now() >= d)
            || (!finished.is_empty() && finished.iter().all(|f| f.load(Ordering::SeqCst)))
        {
            break;
        }
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(100)) {
            print(e);
        }
    }
    let stopped = meeting.stop()?;
    for e in rx.try_iter() {
        print(e);
    }
    let path = stopped.save(args.title)?;
    eprintln!("saved {}", path.display());
    Ok(())
}

pub(crate) fn mic_source() -> Result<SourceFactory> {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    return Ok(Box::new(|| Ok(Box::new(tyst_platform::pipewire::PipeWireSource::microphone()) as _)));
    #[cfg(all(not(all(target_os = "linux", feature = "pipewire")), feature = "mic"))]
    return Ok(Box::new(|| Ok(Box::new(tyst_platform::mic::MicSource::new()) as _)));
    #[allow(unreachable_code)]
    {
        bail!("built without microphone support")
    }
}

fn system_source() -> Result<SourceFactory> {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    return Ok(Box::new(|| Ok(Box::new(tyst_platform::pipewire::PipeWireSource::system_audio()) as _)));
    #[cfg(all(target_os = "macos", feature = "macos-tap"))]
    return Ok(Box::new(|| Ok(Box::new(tyst_platform::macos_tap::SystemAudioTap::new()) as _)));
    #[allow(unreachable_code)]
    {
        bail!("system audio capture is not available in this build")
    }
}
