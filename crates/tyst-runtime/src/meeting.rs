//! A live meeting session (SPEC 6.1, 8.3): one capture source and one pipeline per channel (Me,
//! Others), merged into one transcript by timestamp, journaled as it goes (SPEC 6.6), paused and
//! resumed, and saved as Markdown once the user has named it.
//!
//! Threads: each source runs on its own capture thread (some backends' streams are not `Send`),
//! and each channel has a worker that resamples, segments and decodes. Segment times are offsets
//! from the session start on the wall clock: when capture has a gap (pause, a device switch, a
//! system-audio stream that only runs while something plays) the worker skips its pipeline
//! forward, so Me and Others stay on the same timeline.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;
use tyst_core::journal::Journal;
use tyst_core::markdown;
use tyst_core::pipeline::{ChannelPipeline, PipelineEvent, SharedEngines};
use tyst_core::resample::{Resampler, SAMPLE_RATE};
use tyst_core::router::LanguageMode;
use tyst_core::segmenter::SpeechDetector;
use tyst_core::transcript::{
    Channel, Marker, MarkerKind, Segment, Session, SessionInfo, SpeakerLabels, new_session_id,
};
use tyst_platform::{AudioChunk, AudioSource, CaptureError};

use crate::echo::{Block, EchoCanceller, EchoReference};
use crate::{Result, Runtime};

/// Builds a source on its capture thread.
pub type SourceFactory = Box<dyn FnOnce() -> Result<Box<dyn AudioSource>, CaptureError> + Send>;

/// Capture gaps longer than this end the open segment and move the channel's clock forward.
const GAP: Duration = Duration::from_secs(1);
/// Level meter update interval.
const LEVEL_INTERVAL: usize = SAMPLE_RATE as usize / 10;
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq)]
pub enum MeetingEvent {
    /// Replaces the previous partial of the same segment.
    Partial { channel: Channel, segment_id: u64, text: String },
    /// Final text of a segment (already journaled).
    Final(Segment),
    /// The open segment was too short; drop its partial.
    Dropped { channel: Channel, segment_id: u64 },
    /// RMS level of the last 100 ms (0..1), for the level meters.
    Level { channel: Channel, rms: f32 },
    /// A capture problem; the session keeps running.
    CaptureError { channel: Channel, message: String },
    /// A pipeline or journal problem; the session keeps running.
    Error { channel: Option<Channel>, message: String },
}

pub struct MeetingOptions {
    /// Where the journal and the Markdown file go.
    pub transcripts_dir: PathBuf,
    pub labels: SpeakerLabels,
    /// App name and version for the front matter, e.g. "Tyst 0.1.0".
    pub app: String,
    /// Initial session language.
    pub mode: LanguageMode,
    pub sources: Vec<(Channel, SourceFactory)>,
    /// Cancel the system audio's echo in the microphone (SPEC 6.4) when both channels run.
    pub echo_cancellation: bool,
}

#[derive(Default)]
struct Recorder {
    journal: Option<Journal>,
    segments: Vec<Segment>,
    markers: Vec<Marker>,
}

struct Shared {
    started: Instant,
    paused: AtomicBool,
    /// The user is dictating: the Me channel drops its audio (SPEC 15 q4).
    dictating: AtomicBool,
    /// Session language (SPEC 6.3: Auto / Swedish / English), switchable while recording.
    mode: AtomicU8,
    recorder: Mutex<Recorder>,
    events: Sender<MeetingEvent>,
}

fn mode_to_u8(m: LanguageMode) -> u8 {
    match m {
        LanguageMode::Auto => 0,
        LanguageMode::Swedish => 1,
        LanguageMode::English => 2,
    }
}

fn mode_from_u8(v: u8) -> LanguageMode {
    match v {
        1 => LanguageMode::Swedish,
        2 => LanguageMode::English,
        _ => LanguageMode::Auto,
    }
}

impl Shared {
    /// Stream position (16 kHz samples since the session start) of a wall-clock instant.
    fn position_at(&self, t: Instant) -> u64 {
        (t.saturating_duration_since(self.started).as_secs_f64() * SAMPLE_RATE as f64) as u64
    }

    fn emit(&self, e: MeetingEvent) {
        let _ = self.events.send(e);
    }

    fn record(&self, channel: Channel, events: Vec<PipelineEvent>) {
        for e in events {
            match e {
                PipelineEvent::Partial { segment_id, text, .. } => {
                    self.emit(MeetingEvent::Partial { channel, segment_id, text })
                }
                PipelineEvent::Dropped { segment_id, .. } => self.emit(MeetingEvent::Dropped { channel, segment_id }),
                PipelineEvent::Final { segment, .. } => {
                    let mut rec = self.recorder.lock().expect("recorder lock");
                    if let Some(j) = rec.journal.as_mut()
                        && let Err(e) = j.append(&segment)
                    {
                        log::error!("journal append failed: {e}");
                        self.emit(MeetingEvent::Error { channel: Some(channel), message: e.to_string() });
                    }
                    rec.segments.push(segment.clone());
                    drop(rec);
                    self.emit(MeetingEvent::Final(segment));
                }
            }
        }
    }
}

enum CaptureCmd {
    Pause,
    Resume,
}

struct ChannelThreads {
    channel: Channel,
    control: Sender<CaptureCmd>,
    capture: JoinHandle<()>,
    /// Others with echo cancellation: writes the reference as audio arrives (see [`reference_tee`]).
    tee: Option<JoinHandle<()>>,
    worker: JoinHandle<()>,
}

/// A running meeting.
pub struct Meeting {
    info: SessionInfo,
    shared: Arc<Shared>,
    engines: SharedEngines,
    channels: Vec<ChannelThreads>,
    transcripts_dir: PathBuf,
}

impl Meeting {
    /// Starts capture and transcription on every source. Fails if any source fails to start.
    pub fn start(runtime: &Runtime, opts: MeetingOptions, events: Sender<MeetingEvent>) -> Result<Self> {
        Self::start_with(runtime.engines.clone(), |ch| runtime.pipeline(ch, true), opts, events)
    }

    /// [`start`](Self::start) with pipelines from `pipeline`, which must share `engines`.
    pub fn start_with<D: SpeechDetector + 'static>(
        engines: SharedEngines,
        mut pipeline: impl FnMut(Channel) -> Result<ChannelPipeline<D>>,
        opts: MeetingOptions,
        events: Sender<MeetingEvent>,
    ) -> Result<Self> {
        let started_at = Local::now().fixed_offset();
        let info = SessionInfo {
            id: new_session_id(&started_at),
            started_at,
            app: opts.app,
            models: engines.lock().map(|e| e.loaded_ids()).unwrap_or_default(),
            labels: opts.labels,
        };
        let journal = Journal::create(&opts.transcripts_dir, &info)?;
        log::info!("meeting {} started, {} channel(s)", info.id, opts.sources.len());
        let shared = Arc::new(Shared {
            started: Instant::now(),
            paused: AtomicBool::new(false),
            dictating: AtomicBool::new(false),
            mode: AtomicU8::new(mode_to_u8(opts.mode)),
            recorder: Mutex::new(Recorder { journal: Some(journal), ..Default::default() }),
            events,
        });
        let mut meeting =
            Self { info, shared: shared.clone(), engines, channels: Vec::new(), transcripts_dir: opts.transcripts_dir };
        let both = [Channel::Me, Channel::Others].iter().all(|c| opts.sources.iter().any(|(s, _)| s == c));
        let reference = (opts.echo_cancellation && both).then(|| Arc::new(EchoReference::default()));
        for (channel, factory) in opts.sources {
            let echo = match (&reference, channel) {
                (Some(r), Channel::Others) => Echo::Reference(r.clone()),
                (Some(r), Channel::Me) => match EchoCanceller::new(r.clone()) {
                    Ok(c) => {
                        log::info!("echo cancellation on");
                        Echo::Cancel(Box::new(c))
                    }
                    Err(e) => {
                        log::warn!("echo cancellation unavailable: {e}");
                        Echo::Off
                    }
                },
                _ => Echo::Off,
            };
            let pipeline = match pipeline(channel) {
                Ok(p) => p,
                Err(e) => {
                    meeting.abort();
                    return Err(e);
                }
            };
            match spawn_channel(channel, factory, pipeline, echo, shared.clone()) {
                Ok(t) => meeting.channels.push(t),
                Err(e) => {
                    meeting.abort();
                    return Err(e.into());
                }
            }
        }
        Ok(meeting)
    }

    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn elapsed(&self) -> Duration {
        self.shared.started.elapsed()
    }

    pub fn language(&self) -> LanguageMode {
        mode_from_u8(self.shared.mode.load(Ordering::SeqCst))
    }

    /// Switches the session language; applies from the next segment on every channel.
    pub fn set_language(&self, mode: LanguageMode) {
        self.shared.mode.store(mode_to_u8(mode), Ordering::SeqCst);
        log::info!("meeting language: {mode:?}");
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::SeqCst)
    }

    /// Stops capture on every channel and writes a Paused marker (SPEC 8.3). Open segments are
    /// finalized.
    pub fn pause(&mut self) -> Result<()> {
        if self.shared.paused.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let marker = Marker { at: self.elapsed(), kind: MarkerKind::Paused };
        {
            let mut rec = self.shared.recorder.lock().expect("recorder lock");
            if let Some(j) = rec.journal.as_mut() {
                j.mark(marker)?;
            }
            rec.markers.push(marker);
        }
        for c in &self.channels {
            let _ = c.control.send(CaptureCmd::Pause);
        }
        log::info!("meeting paused at {:.1}s", marker.at.as_secs_f32());
        Ok(())
    }

    /// While dictation runs, the Me channel leaves its audio out of the meeting (the dictated
    /// text goes to another app, SPEC 8.4) and a Dictating marker takes its place.
    pub fn set_dictating(&self, on: bool) -> Result<()> {
        if self.shared.dictating.swap(on, Ordering::SeqCst) == on || !on {
            return Ok(());
        }
        let marker = Marker { at: self.elapsed(), kind: MarkerKind::Dictating };
        let mut rec = self.shared.recorder.lock().expect("recorder lock");
        if let Some(j) = rec.journal.as_mut() {
            j.mark(marker)?;
        }
        rec.markers.push(marker);
        Ok(())
    }

    pub fn resume(&mut self) {
        if !self.shared.paused.swap(false, Ordering::SeqCst) {
            return;
        }
        for c in &self.channels {
            let _ = c.control.send(CaptureCmd::Resume);
        }
        log::info!("meeting resumed at {:.1}s", self.elapsed().as_secs_f32());
    }

    /// Stops capture, decodes what is still open and returns the finished session, still
    /// journaled, waiting for a title.
    pub fn stop(mut self) -> Result<StoppedMeeting> {
        let ended_at = Local::now().fixed_offset();
        self.join();
        let mut rec = std::mem::take(&mut *self.shared.recorder.lock().expect("recorder lock"));
        let mut journal = rec.journal.take();
        if let Some(j) = journal.as_mut() {
            j.end(ended_at)?;
        }
        let mut info = self.info.clone();
        info.models = self.engines.lock().map(|e| e.loaded_ids()).unwrap_or(info.models);
        log::info!("meeting {} stopped: {} segments", info.id, rec.segments.len());
        Ok(StoppedMeeting {
            session: Session { info, ended_at, title: None, segments: rec.segments, markers: rec.markers },
            journal,
            transcripts_dir: self.transcripts_dir.clone(),
        })
    }

    fn join(&mut self) {
        for c in self.channels.drain(..) {
            drop(c.control); // ends the capture thread, which drops the source and its sender
            if c.capture.join().is_err() {
                log::error!("{:?} capture thread panicked", c.channel);
            }
            if let Some(tee) = c.tee
                && tee.join().is_err()
            {
                log::error!("{:?} echo reference thread panicked", c.channel);
            }
            if c.worker.join().is_err() {
                log::error!("{:?} worker thread panicked", c.channel);
            }
        }
    }

    /// Start failed: stop what runs and delete the empty journal.
    fn abort(&mut self) {
        self.join();
        if let Some(j) = self.shared.recorder.lock().expect("recorder lock").journal.take() {
            let _ = j.remove();
        }
    }
}

impl Drop for Meeting {
    fn drop(&mut self) {
        // Dropped without stop(): stop the threads; the journal stays for recovery.
        self.join();
    }
}

/// A stopped meeting waiting to be saved. Dropping it without saving leaves the journal on disk,
/// so the meeting can be recovered on the next launch.
pub struct StoppedMeeting {
    session: Session,
    journal: Option<Journal>,
    transcripts_dir: PathBuf,
}

impl StoppedMeeting {
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The file name `save` would use for this title, for the stop prompt's path preview.
    pub fn preview_path(&self, title: Option<&str>) -> PathBuf {
        let mut s = self.session.clone();
        s.title = title.map(String::from);
        self.transcripts_dir.join(markdown::file_name(&s))
    }

    /// Writes the Markdown file (never overwriting) and deletes the journal.
    pub fn save(mut self, title: Option<String>) -> Result<PathBuf> {
        let title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        if let (Some(j), Some(t)) = (self.journal.as_mut(), title.as_deref()) {
            j.set_title(t)?;
        }
        self.session.title = title;
        let path = markdown::save(&self.session, &self.transcripts_dir)?;
        if let Some(j) = self.journal.take() {
            j.remove()?;
        }
        log::info!("meeting {} saved", self.session.info.id);
        Ok(path)
    }

    pub fn transcripts_dir(&self) -> &Path {
        &self.transcripts_dir
    }
}

/// A channel's part in echo cancellation.
enum Echo {
    Off,
    /// Others: writes the system audio as the reference (on its own thread, [`reference_tee`]).
    Reference(Arc<EchoReference>),
    /// Me: cancels the reference's echo before the pipeline.
    Cancel(Box<EchoCanceller>),
}

fn spawn_channel<D: SpeechDetector + 'static>(
    channel: Channel,
    factory: SourceFactory,
    pipeline: ChannelPipeline<D>,
    echo: Echo,
    shared: Arc<Shared>,
) -> Result<ChannelThreads, CaptureError> {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>();
    let (control, control_rx) = mpsc::channel::<CaptureCmd>();
    let (started_tx, started_rx) = mpsc::sync_channel::<Result<String, CaptureError>>(1);
    // The worker spends time decoding; the echo reference must not wait for it, or Me (which
    // waits for the reference) would fall behind and cancel against silence.
    let (source_tx, tee, echo) = match echo {
        Echo::Reference(reference) => {
            let (tx, rx) = mpsc::channel::<AudioChunk>();
            let shared = shared.clone();
            let tee = std::thread::Builder::new()
                .name(format!("tyst-echo-ref-{channel:?}"))
                .spawn(move || reference_tee(rx, audio_tx, reference, shared))
                .map_err(|e| CaptureError::Device(e.to_string()))?;
            (tx, Some(tee), Echo::Off)
        }
        other => (audio_tx, None, other),
    };
    let capture = {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name(format!("tyst-capture-{channel:?}"))
            .spawn(move || capture_thread(channel, factory, source_tx, control_rx, started_tx, shared))
            .map_err(|e| CaptureError::Device(e.to_string()))?
    };
    let worker = std::thread::Builder::new()
        .name(format!("tyst-worker-{channel:?}"))
        .spawn(move || worker_thread(channel, pipeline, echo, audio_rx, shared))
        .map_err(|e| CaptureError::Device(e.to_string()))?;
    let threads = ChannelThreads { channel, control, capture, tee, worker };
    match started_rx.recv() {
        Ok(Ok(device)) => {
            log::info!("{channel:?} capture started on {device}");
            Ok(threads)
        }
        Ok(Err(e)) => {
            let _ = threads.capture.join();
            if let Some(tee) = threads.tee {
                let _ = tee.join();
            }
            let _ = threads.worker.join();
            Err(e)
        }
        Err(_) => Err(CaptureError::Device("capture thread exited".into())),
    }
}

/// Writes system audio into the echo reference as soon as it is captured, then hands it on to
/// the Others worker.
fn reference_tee(
    rx: Receiver<AudioChunk>,
    worker: Sender<AudioChunk>,
    reference: Arc<EchoReference>,
    shared: Arc<Shared>,
) {
    let mut resampler: Option<(u32, Resampler)> = None;
    for chunk in rx {
        let r = match &mut resampler {
            Some((rate, r)) if *rate == chunk.sample_rate => r,
            _ => &mut resampler.insert((chunk.sample_rate, Resampler::new(chunk.sample_rate, SAMPLE_RATE))).1,
        };
        let pcm = r.push(&chunk.samples);
        if !pcm.is_empty() {
            let at = shared.position_at(chunk.captured_at).saturating_sub(pcm.len() as u64);
            reference.push(at, &pcm);
        }
        if worker.send(chunk).is_err() {
            break;
        }
    }
}

fn capture_thread(
    channel: Channel,
    factory: SourceFactory,
    sink: Sender<AudioChunk>,
    control: Receiver<CaptureCmd>,
    started: mpsc::SyncSender<Result<String, CaptureError>>,
    shared: Arc<Shared>,
) {
    let mut source = match factory().and_then(|mut s| s.start(sink.clone()).map(|_| s)) {
        Ok(s) => {
            let _ = started.send(Ok(s.device_name()));
            s
        }
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let mut running = true;
    loop {
        match control.recv_timeout(POLL_INTERVAL) {
            Ok(CaptureCmd::Pause) if running => {
                if let Err(e) = source.stop() {
                    shared.emit(MeetingEvent::CaptureError { channel, message: e.to_string() });
                }
                running = false;
            }
            Ok(CaptureCmd::Resume) if !running => match source.start(sink.clone()) {
                Ok(()) => running = true,
                Err(e) => shared.emit(MeetingEvent::CaptureError { channel, message: e.to_string() }),
            },
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if running && let Err(e) = source.poll() {
                    log::warn!("{channel:?} capture: {e}");
                    shared.emit(MeetingEvent::CaptureError { channel, message: e.to_string() });
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = source.stop();
}

fn worker_thread<D: SpeechDetector>(
    channel: Channel,
    pipeline: ChannelPipeline<D>,
    mut echo: Echo,
    rx: Receiver<AudioChunk>,
    shared: Arc<Shared>,
) {
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut flushed_for_pause = false;
    let mut feed = Feed::new(channel, pipeline, shared.clone());
    loop {
        let chunk = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                if let Echo::Cancel(aec) = &mut echo {
                    feed.blocks(aec.poll(shared.position_at(Instant::now())));
                }
                // Paused: finalize the open segment now rather than when capture resumes.
                if shared.paused.load(Ordering::SeqCst) && !flushed_for_pause {
                    flushed_for_pause = true;
                    if let Echo::Cancel(aec) = &mut echo {
                        feed.blocks(aec.flush());
                    }
                    feed.skip_to(shared.position_at(Instant::now()));
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        flushed_for_pause = false;
        let r = match &mut resampler {
            Some((rate, r)) if *rate == chunk.sample_rate => r,
            _ => &mut resampler.insert((chunk.sample_rate, Resampler::new(chunk.sample_rate, SAMPLE_RATE))).1,
        };
        let pcm = r.push(&chunk.samples);
        if pcm.is_empty() {
            continue;
        }
        // Where this audio belongs on the session clock; a gap means capture was interrupted.
        let at = shared.position_at(chunk.captured_at).saturating_sub(pcm.len() as u64);
        match &mut echo {
            Echo::Off | Echo::Reference(_) => feed.block(at, &pcm),
            Echo::Cancel(aec) => feed.blocks(aec.push(at, &pcm, shared.position_at(Instant::now()))),
        }
    }
    if let Echo::Cancel(aec) = &mut echo {
        feed.blocks(aec.flush());
        if let Some(ms) = aec.delay_ms() {
            log::info!("echo delay at the end: {ms} ms");
        }
    }
    feed.flush();
}

/// Feeds a channel's 16 kHz audio into its pipeline: language switches, dictation, capture gaps
/// and the level meter.
struct Feed<D: SpeechDetector> {
    channel: Channel,
    pipeline: ChannelPipeline<D>,
    shared: Arc<Shared>,
    mode: LanguageMode,
    dictating: bool,
    level_sum: f64,
    level_n: usize,
}

impl<D: SpeechDetector> Feed<D> {
    fn new(channel: Channel, mut pipeline: ChannelPipeline<D>, shared: Arc<Shared>) -> Self {
        let mode = pipeline.router_mut().mode();
        Self { channel, pipeline, shared, mode, dictating: false, level_sum: 0.0, level_n: 0 }
    }

    fn record(&self, r: tyst_core::Result<Vec<PipelineEvent>>) {
        match r {
            Ok(ev) => self.shared.record(self.channel, ev),
            Err(e) => {
                log::error!("{:?} pipeline: {e}", self.channel);
                self.shared.emit(MeetingEvent::Error { channel: Some(self.channel), message: e.to_string() });
            }
        }
    }

    fn skip_to(&mut self, at: u64) {
        let r = self.pipeline.skip_to(at);
        self.record(r);
    }

    fn blocks(&mut self, blocks: Vec<Block>) {
        for (at, pcm) in blocks {
            self.block(at, &pcm);
        }
    }

    /// Audio that starts at session position `at`.
    fn block(&mut self, at: u64, pcm: &[f32]) {
        let wanted = mode_from_u8(self.shared.mode.load(Ordering::SeqCst));
        if wanted != self.mode {
            self.mode = wanted;
            self.pipeline.router_mut().set_mode(wanted);
        }
        // Dictation: Me drops its audio, ending the open segment and moving its clock along.
        let now_dictating = self.channel == Channel::Me && self.shared.dictating.load(Ordering::SeqCst);
        if now_dictating || self.dictating {
            self.skip_to(at);
            self.dictating = now_dictating;
            if self.dictating {
                return;
            }
        }
        let gap = (GAP.as_secs_f64() * SAMPLE_RATE as f64) as u64;
        if at > self.pipeline.position() + gap {
            log::info!(
                "{:?}: capture gap of {:.1}s",
                self.channel,
                (at - self.pipeline.position()) as f64 / SAMPLE_RATE as f64
            );
            self.skip_to(at);
        }
        let r = self.pipeline.push(pcm);
        self.record(r);
        for &x in pcm {
            self.level_sum += (x * x) as f64;
            self.level_n += 1;
            if self.level_n == LEVEL_INTERVAL {
                let rms = (self.level_sum / self.level_n as f64).sqrt() as f32;
                self.shared.emit(MeetingEvent::Level { channel: self.channel, rms });
                self.level_sum = 0.0;
                self.level_n = 0;
            }
        }
    }

    fn flush(&mut self) {
        let r = self.pipeline.flush();
        self.record(r);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use tyst_core::asr::{AsrEngine, AsrResult};
    use tyst_core::pipeline::{EngineSet, PipelineConfig};
    use tyst_core::router::FixedRouter;
    use tyst_core::segmenter::EnergyDetector;
    use tyst_core::vocabulary::VocabularyRules;

    use super::*;

    struct FakeEngine;

    impl AsrEngine for FakeEngine {
        fn id(&self) -> &str {
            "fake@1"
        }
        fn transcribe(&mut self, pcm: &[f32]) -> tyst_core::Result<AsrResult> {
            Ok(AsrResult { text: format!("{:.1} sekunder", pcm.len() as f32 / 16_000.0), ..Default::default() })
        }
    }

    /// Plays (seconds, loud?) pieces at 48 kHz in 20 ms chunks, `offset` later on the clock than
    /// real time (to fake a capture gap), as fast as the worker takes them (or in real time when
    /// `paced`).
    struct FakeSource {
        channel: Channel,
        pattern: Vec<(f32, bool)>,
        offset: Duration,
        paced: bool,
        thread: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        starts: Arc<StdMutex<usize>>,
    }

    impl AudioSource for FakeSource {
        fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
            let mut n = self.starts.lock().unwrap();
            *n += 1;
            if *n > 1 {
                return Ok(()); // resumed: nothing more to play
            }
            let (channel, pattern, offset, stop, paced) =
                (self.channel, self.pattern.clone(), self.offset, self.stop.clone(), self.paced);
            self.thread = Some(std::thread::spawn(move || {
                let t0 = Instant::now() + offset;
                let mut played = 0usize;
                for (secs, loud) in pattern {
                    for _ in 0..(secs * 50.0) as usize {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        let samples: Vec<f32> = (0..960)
                            .map(|i| ((played + i) as f32 * 0.0167).sin() * if loud { 0.3 } else { 0.001 })
                            .collect();
                        played += 960;
                        let at = t0 + Duration::from_secs_f64(played as f64 / 48_000.0);
                        if paced && let Some(d) = at.checked_duration_since(Instant::now()) {
                            std::thread::sleep(d);
                        }
                        if sink.send(AudioChunk { channel, sample_rate: 48_000, samples, captured_at: at }).is_err() {
                            return;
                        }
                    }
                }
            }));
            Ok(())
        }
        fn stop(&mut self) -> Result<(), CaptureError> {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            Ok(())
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn device_name(&self) -> String {
            "fake".into()
        }
    }

    fn source(channel: Channel, pattern: Vec<(f32, bool)>, offset: Duration) -> (Channel, SourceFactory) {
        source_with(channel, pattern, offset, false)
    }

    fn source_with(
        channel: Channel,
        pattern: Vec<(f32, bool)>,
        offset: Duration,
        paced: bool,
    ) -> (Channel, SourceFactory) {
        let f: SourceFactory = Box::new(move || {
            Ok(Box::new(FakeSource {
                channel,
                pattern,
                offset,
                paced,
                thread: None,
                stop: Arc::new(AtomicBool::new(false)),
                starts: Arc::new(StdMutex::new(0)),
            }) as Box<dyn AudioSource>)
        });
        (channel, f)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tyst-meeting-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn start(dir: &Path, sources: Vec<(Channel, SourceFactory)>) -> (Meeting, Receiver<MeetingEvent>) {
        let engines = EngineSet::new().with_primary(Box::new(FakeEngine)).shared();
        let (tx, rx) = mpsc::channel();
        let e2 = engines.clone();
        let meeting = Meeting::start_with(
            engines,
            move |ch| {
                Ok(ChannelPipeline::new(
                    ch,
                    PipelineConfig::default(),
                    EnergyDetector { window: 512, full_scale_rms: 0.1 },
                    e2.clone(),
                    Box::new(FixedRouter::default()),
                    Arc::new(VocabularyRules::default()),
                ))
            },
            MeetingOptions {
                transcripts_dir: dir.to_path_buf(),
                labels: SpeakerLabels::default(),
                app: "Tyst test".into(),
                mode: LanguageMode::Auto,
                sources,
                echo_cancellation: true,
            },
            tx,
        )
        .unwrap();
        (meeting, rx)
    }

    #[test]
    fn two_channels_on_one_timeline_journaled_then_saved() {
        let dir = tmp("two");
        let (meeting, rx) = start(
            &dir,
            vec![
                source(Channel::Me, vec![(0.5, false), (2.0, true), (1.0, false)], Duration::ZERO),
                // Others starts 6 s later on the session clock (system audio idle until then).
                source(Channel::Others, vec![(0.5, false), (3.0, true), (1.0, false)], Duration::from_secs(6)),
            ],
        );
        std::thread::sleep(Duration::from_millis(300));
        // While running, the journal holds the session.
        assert_eq!(tyst_core::journal::find_orphans(&dir).unwrap().len(), 1);
        let stopped = meeting.stop().unwrap();
        let mut segs = stopped.session().segments.clone();
        segs.sort_by_key(|s| s.start);
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert_eq!(segs[0].channel, Channel::Me);
        assert!(segs[0].start < Duration::from_secs(1), "{:?}", segs[0].start);
        assert_eq!(segs[1].channel, Channel::Others);
        assert!(
            segs[1].start > Duration::from_secs(6) && segs[1].start < Duration::from_secs(7),
            "{:?}",
            segs[1].start
        );
        let events: Vec<MeetingEvent> = rx.try_iter().collect();
        assert_eq!(events.iter().filter(|e| matches!(e, MeetingEvent::Final(_))).count(), 2);
        assert!(events.iter().any(|e| matches!(e, MeetingEvent::Partial { channel: Channel::Others, .. })));
        assert!(events.iter().any(|e| matches!(e, MeetingEvent::Level { rms, .. } if *rms > 0.1)));

        let preview = stopped.preview_path(Some("Sync"));
        let path = stopped.save(Some(" Sync ".into())).unwrap();
        assert_eq!(path, preview);
        let md = std::fs::read_to_string(&path).unwrap();
        assert!(md.contains("**Me:** 2.") && md.contains("**Others:** 3."), "{md}");
        assert!(tyst_core::journal::find_orphans(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pause_writes_a_marker_and_dropping_keeps_the_journal() {
        let dir = tmp("pause");
        let (mut meeting, _rx) =
            start(&dir, vec![source(Channel::Me, vec![(0.5, false), (1.0, true)], Duration::ZERO)]);
        std::thread::sleep(Duration::from_millis(200));
        meeting.pause().unwrap();
        assert!(meeting.is_paused());
        std::thread::sleep(Duration::from_millis(300));
        meeting.resume();
        assert!(!meeting.is_paused());
        let stopped = meeting.stop().unwrap();
        assert_eq!(stopped.session().markers.len(), 1);
        // Not saved (e.g. a crash during the name prompt): the journal is recoverable.
        drop(stopped);
        let orphans = tyst_core::journal::find_orphans(&dir).unwrap();
        assert_eq!(orphans.len(), 1);
        let recovered = tyst_core::journal::recover(&orphans[0]).unwrap();
        assert_eq!(recovered.markers.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dictation_leaves_me_out_and_writes_a_marker() {
        let dir = tmp("dictating");
        // Me speaks from 0.5 s to 3.5 s; dictation covers 1.0 s to 2.5 s of it.
        let me = source_with(Channel::Me, vec![(0.5, false), (3.0, true), (1.0, false)], Duration::ZERO, true);
        let (meeting, _rx) = start(&dir, vec![me]);
        std::thread::sleep(Duration::from_millis(1000));
        meeting.set_dictating(true).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        meeting.set_dictating(false).unwrap();
        std::thread::sleep(Duration::from_millis(1600));
        let stopped = meeting.stop().unwrap();
        let s = stopped.session();
        assert_eq!(s.markers.len(), 1);
        assert_eq!(s.markers[0].kind, MarkerKind::Dictating);
        let total: f32 = s.segments.iter().map(|x| (x.end - x.start).as_secs_f32()).sum();
        assert!(total < 2.0, "dictated audio left out: {:?}", s.segments);
        for seg in &s.segments {
            assert!(seg.end <= Duration::from_millis(1200) || seg.start >= Duration::from_millis(2400), "{seg:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failing_source_fails_start_and_removes_the_journal() {
        let dir = tmp("fail");
        let engines = EngineSet::new().with_primary(Box::new(FakeEngine)).shared();
        let (tx, _rx) = mpsc::channel();
        let e2 = engines.clone();
        let bad: SourceFactory = Box::new(|| Err(CaptureError::NoDevice));
        let r = Meeting::start_with(
            engines,
            move |ch| {
                Ok(ChannelPipeline::new(
                    ch,
                    PipelineConfig::default(),
                    EnergyDetector { window: 512, full_scale_rms: 0.1 },
                    e2.clone(),
                    Box::new(FixedRouter::default()),
                    Arc::new(VocabularyRules::default()),
                ))
            },
            MeetingOptions {
                transcripts_dir: dir.clone(),
                labels: SpeakerLabels::default(),
                app: "Tyst test".into(),
                mode: LanguageMode::Auto,
                sources: vec![(Channel::Me, bad)],
                echo_cancellation: false,
            },
            tx,
        );
        assert!(r.is_err());
        assert!(tyst_core::journal::find_orphans(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
