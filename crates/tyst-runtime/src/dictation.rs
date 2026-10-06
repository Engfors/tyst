//! A dictation session (SPEC 8.4): the microphone through one channel pipeline while the user
//! holds or toggles the shortcut, live text for the pill, and the final text as soon as they stop.
//!
//! Latency is what counts here (Phase 3: end of speech -> text ≤ 1 s): segments the VAD closes
//! during pauses are decoded while the user keeps talking, so stopping leaves only the open tail
//! to decode, and partial decodes stop the moment the user does. The captured audio is kept (up
//! to [`MAX_KEPT_AUDIO`]) so the pill can re-transcribe it in another language.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tyst_core::pipeline::{ChannelPipeline, PipelineEvent};
use tyst_core::resample::{Resampler, SAMPLE_RATE};
use tyst_core::router::LanguageMode;
use tyst_core::segmenter::SpeechDetector;
use tyst_core::transcript::Lang;
use tyst_platform::{AudioChunk, CaptureError};

use crate::meeting::SourceFactory;
use crate::queue::{Queue, chunk_ms};
use crate::{Error, Result};

/// Audio kept for re-transcription; longer dictations keep only their first five minutes.
pub const MAX_KEPT_AUDIO: Duration = Duration::from_secs(300);
/// With no segment found by the VAD, audio at least this long and this loud is decoded whole:
/// the user bounded it by hand, so quiet or very short speech still gets a try.
const FALLBACK_MIN_AUDIO: Duration = Duration::from_millis(300);
const FALLBACK_MIN_PEAK: f32 = 0.02;
const LEVEL_INTERVAL: usize = SAMPLE_RATE as usize / 20;
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq)]
pub enum DictationEvent {
    /// Text so far: `committed` is final, `partial` the open segment's latest guess.
    Text {
        committed: String,
        partial: String,
    },
    /// RMS level of the last 50 ms (0..1), for the pill's waveform.
    Level {
        rms: f32,
    },
    /// The microphone failed; the session keeps running and may recover.
    CaptureError {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct DictationOptions {
    pub mode: LanguageMode,
    /// Language of the last dictation: short utterances keep it (SPEC 8.4).
    pub initial: Lang,
}

/// The outcome of a finished dictation.
#[derive(Debug, Clone, PartialEq)]
pub struct Dictated {
    /// Final text, single-spaced, without leading or trailing whitespace (SPEC 8.4: nothing is
    /// ever appended, so nothing runs in a terminal by accident).
    pub text: String,
    pub lang: Lang,
    /// 16 kHz audio as captured, for [`retranscribe`].
    pub audio: Vec<f32>,
    /// Length of the whole recording.
    pub duration: Duration,
    /// Speech segments decoded.
    pub segments: usize,
    /// From [`Dictation::finish`] being called to the text being ready.
    pub finish_latency: Duration,
    /// Decode time spent after `finish` (the open tail).
    pub tail_decode: Duration,
}

struct Shared {
    mode: AtomicU8,
    finishing: AtomicBool,
    cancelled: AtomicBool,
    events: Sender<DictationEvent>,
    /// When `finish` was called.
    finish_at: Mutex<Option<Instant>>,
}

impl Shared {
    fn emit(&self, e: DictationEvent) {
        let _ = self.events.send(e);
    }
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

/// A running dictation.
pub struct Dictation {
    shared: Arc<Shared>,
    control: Option<Sender<()>>,
    capture: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<Result<Dictated>>>,
    device: String,
}

impl Dictation {
    /// Starts capture from `source` into `pipeline` (which should have partials on). Returns once
    /// the source runs, or its error.
    pub fn start<D: SpeechDetector + 'static>(
        pipeline: ChannelPipeline<D>,
        source: SourceFactory,
        opts: DictationOptions,
        events: Sender<DictationEvent>,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            mode: AtomicU8::new(mode_to_u8(opts.mode)),
            finishing: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            events,
            finish_at: Mutex::new(None),
        });
        let (audio_tx, audio_rx) = mpsc::channel::<AudioChunk>();
        // Milliseconds of audio sent to the worker and not yet taken: what keeps the queue bounded.
        let backlog = Arc::new(AtomicU64::new(0));
        let (control, control_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::sync_channel::<Result<String, CaptureError>>(1);
        let capture = {
            let shared = shared.clone();
            let backlog = backlog.clone();
            std::thread::Builder::new()
                .name("tyst-dictation-capture".into())
                .spawn(move || {
                    let on_behind = {
                        let shared = shared.clone();
                        Box::new(move || {
                            shared.emit(DictationEvent::CaptureError {
                                message: "Transcription is falling behind; some audio is skipped.".into(),
                            })
                        })
                    };
                    let queue = Queue::new("dictation".into(), audio_tx, backlog, on_behind);
                    capture_thread(source, queue, control_rx, started_tx, shared)
                })
                .map_err(|e| Error::Other(e.to_string()))?
        };
        let worker = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("tyst-dictation-worker".into())
                .spawn(move || worker_thread(pipeline, audio_rx, backlog, shared))
                .map_err(|e| Error::Other(e.to_string()))?
        };
        let device = match started_rx.recv() {
            Ok(Ok(device)) => device,
            Ok(Err(e)) => {
                shared.cancelled.store(true, Ordering::SeqCst);
                let _ = capture.join();
                let _ = worker.join();
                return Err(e.into());
            }
            Err(_) => return Err(Error::Other("capture thread exited".into())),
        };
        log::info!("dictation started on {device}");
        Ok(Self { shared, control: Some(control), capture: Some(capture), worker: Some(worker), device })
    }

    pub fn device_name(&self) -> &str {
        &self.device
    }

    /// Switches the language; applies from the next segment.
    pub fn set_language(&self, mode: LanguageMode) {
        self.shared.mode.store(mode_to_u8(mode), Ordering::SeqCst);
    }

    pub fn language(&self) -> LanguageMode {
        mode_from_u8(self.shared.mode.load(Ordering::SeqCst))
    }

    /// Stops capture and returns the final text once the open tail is decoded.
    pub fn finish(mut self) -> Result<Dictated> {
        *self.shared.finish_at.lock().expect("finish lock") = Some(Instant::now());
        self.shared.finishing.store(true, Ordering::SeqCst);
        self.stop_capture();
        let worker = self.worker.take().expect("worker runs until finish or cancel");
        worker.join().map_err(|_| Error::Other("dictation worker panicked".into()))?
    }

    /// Stops capture and throws everything away (Esc).
    pub fn cancel(mut self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        self.stop_capture();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        log::info!("dictation cancelled");
    }

    fn stop_capture(&mut self) {
        drop(self.control.take()); // ends the capture thread, which drops the source and its sender
        if let Some(c) = self.capture.take()
            && c.join().is_err()
        {
            log::error!("dictation capture thread panicked");
        }
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
        self.stop_capture();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn capture_thread(
    factory: SourceFactory,
    mut queue: Queue,
    control: Receiver<()>,
    started: mpsc::SyncSender<Result<String, CaptureError>>,
    shared: Arc<Shared>,
) {
    let (sink, raw) = mpsc::channel::<AudioChunk>();
    let mut source = match factory().and_then(|mut s| s.start(sink).map(|_| s)) {
        Ok(s) => {
            let _ = started.send(Ok(s.device_name()));
            s
        }
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    loop {
        if let Ok(chunk) = raw.recv_timeout(Duration::from_millis(20)) {
            queue.push(chunk);
            for chunk in raw.try_iter() {
                queue.push(chunk);
            }
        }
        match control.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
        if Instant::now() >= next_poll {
            next_poll = Instant::now() + POLL_INTERVAL;
            if let Err(e) = source.poll() {
                log::warn!("dictation capture: {e}");
                shared.emit(DictationEvent::CaptureError { message: e.to_string() });
            }
        }
    }
    let _ = source.stop();
    drop(source);
    for chunk in raw.try_iter() {
        queue.push(chunk);
    }
}

/// Running text: finals in order plus the open segment's partial.
#[derive(Default)]
struct Text {
    finals: Vec<(u64, String)>,
    partial: Option<(u64, String)>,
}

impl Text {
    fn committed(&self) -> String {
        join(self.finals.iter().map(|(_, t)| t.as_str()))
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts.flat_map(str::split_whitespace).collect::<Vec<_>>().join(" ")
}

fn worker_thread<D: SpeechDetector>(
    mut pipeline: ChannelPipeline<D>,
    rx: Receiver<AudioChunk>,
    backlog: Arc<AtomicU64>,
    shared: Arc<Shared>,
) -> Result<Dictated> {
    let max_kept = (MAX_KEPT_AUDIO.as_secs_f64() * SAMPLE_RATE as f64) as usize;
    let mut resampler: Option<(u32, Resampler)> = None;
    let mut audio: Vec<f32> = Vec::new();
    let mut total = 0u64;
    let mut peak = 0.0f32;
    let mut text = Text::default();
    // Longest segment decides the language label.
    let mut langs: Vec<(Duration, Lang)> = Vec::new();
    let mut mode = pipeline.router_mut().mode();
    let mut partials_on = true;
    let (mut level_sum, mut level_n) = (0.0f64, 0usize);
    let mut tail_decode = Duration::ZERO;

    let mut handle = |events: Vec<PipelineEvent>, text: &mut Text, tail_decode: &mut Duration, finishing: bool| {
        let mut changed = false;
        for e in events {
            match e {
                PipelineEvent::Partial { segment_id, text: t, .. } => {
                    text.partial = Some((segment_id, t));
                    changed = true;
                }
                PipelineEvent::Final { segment, stats, .. } => {
                    if finishing {
                        *tail_decode += stats.elapsed;
                    }
                    text.partial = text.partial.take().filter(|(id, _)| *id != segment.id);
                    langs.push((segment.end - segment.start, segment.lang));
                    text.finals.push((segment.id, segment.text));
                    changed = true;
                }
                PipelineEvent::Dropped { segment_id, .. } => {
                    text.partial = text.partial.take().filter(|(id, _)| *id != segment_id);
                    changed = true;
                }
            }
        }
        if changed && !finishing {
            let partial = text.partial.as_ref().map(|(_, t)| t.trim().to_string()).unwrap_or_default();
            shared.emit(DictationEvent::Text { committed: text.committed(), partial });
        }
    };

    loop {
        let chunk = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        backlog.fetch_sub(chunk_ms(&chunk), Ordering::SeqCst);
        if shared.cancelled.load(Ordering::SeqCst) {
            continue; // drain without decoding
        }
        let finishing = shared.finishing.load(Ordering::SeqCst);
        if finishing && partials_on {
            // The user stopped: what is still queued goes straight to the final decode.
            pipeline.set_partials(false);
            partials_on = false;
        }
        let wanted = mode_from_u8(shared.mode.load(Ordering::SeqCst));
        if wanted != mode {
            mode = wanted;
            pipeline.router_mut().set_mode(mode);
        }
        let r = match &mut resampler {
            Some((rate, r)) if *rate == chunk.sample_rate => r,
            _ => &mut resampler.insert((chunk.sample_rate, Resampler::new(chunk.sample_rate, SAMPLE_RATE))).1,
        };
        let pcm = r.push(&chunk.samples);
        if pcm.is_empty() {
            continue;
        }
        total += pcm.len() as u64;
        if audio.len() < max_kept {
            let room = max_kept - audio.len();
            audio.extend_from_slice(&pcm[..pcm.len().min(room)]);
        }
        match pipeline.push(&pcm) {
            Ok(ev) => handle(ev, &mut text, &mut tail_decode, finishing),
            Err(e) => {
                log::error!("dictation pipeline: {e}");
                shared.emit(DictationEvent::Error { message: e.to_string() });
            }
        }
        for &x in &pcm {
            peak = peak.max(x.abs());
            level_sum += (x * x) as f64;
            level_n += 1;
            if level_n == LEVEL_INTERVAL {
                shared.emit(DictationEvent::Level { rms: (level_sum / level_n as f64).sqrt() as f32 });
                level_sum = 0.0;
                level_n = 0;
            }
        }
    }
    if shared.cancelled.load(Ordering::SeqCst) {
        return Err(Error::Other("dictation cancelled".into()));
    }
    // Capture ended on its own before any audio came (the source failed to start).
    if total == 0 && !shared.finishing.load(Ordering::SeqCst) {
        return Err(Error::Other("capture ended before any audio arrived".into()));
    }
    let wanted = mode_from_u8(shared.mode.load(Ordering::SeqCst));
    pipeline.router_mut().set_mode(wanted);
    let t0 = Instant::now();
    let flushed = pipeline.flush()?;
    handle(flushed, &mut text, &mut tail_decode, true);
    let duration = Duration::from_secs_f64(total as f64 / SAMPLE_RATE as f64);
    if text.finals.is_empty() && duration >= FALLBACK_MIN_AUDIO && peak >= FALLBACK_MIN_PEAK {
        let result = pipeline.decode_all(&audio)?;
        if !result.text.trim().is_empty() {
            let lang = pipeline.router_mut().label(&result.text, duration);
            langs.push((duration, lang));
            text.finals.push((0, result.text));
        }
        tail_decode += t0.elapsed();
    }
    let lang = langs.iter().max_by_key(|(d, _)| *d).map(|(_, l)| *l).unwrap_or(if wanted == LanguageMode::English {
        Lang::En
    } else {
        Lang::Sv
    });
    let finish_latency = shared.finish_at.lock().expect("finish lock").map(|t| t.elapsed()).unwrap_or_default();
    let dictated = Dictated {
        text: text.committed(),
        lang,
        audio,
        duration,
        segments: text.finals.len(),
        finish_latency,
        tail_decode,
    };
    log::info!(
        "dictation finished: {:.1}s audio, {} segment(s), {} chars, stop->text {} ms (tail decode {} ms)",
        duration.as_secs_f32(),
        dictated.segments,
        dictated.text.chars().count(),
        finish_latency.as_millis(),
        tail_decode.as_millis()
    );
    Ok(dictated)
}

/// Transcribes kept dictation audio again, e.g. in another language (Tab in the pill). The
/// pipeline should be built for the wanted language, without partials.
pub fn retranscribe<D: SpeechDetector>(mut pipeline: ChannelPipeline<D>, audio: &[f32]) -> Result<(String, Lang)> {
    pipeline.set_partials(false);
    let mut finals = Vec::new();
    let mut langs: Vec<(Duration, Lang)> = Vec::new();
    let mut collect = |events: Vec<PipelineEvent>| {
        for e in events {
            if let PipelineEvent::Final { segment, .. } = e {
                langs.push((segment.end - segment.start, segment.lang));
                finals.push(segment.text);
            }
        }
    };
    for piece in audio.chunks(SAMPLE_RATE as usize / 10) {
        collect(pipeline.push(piece)?);
    }
    collect(pipeline.flush()?);
    let duration = Duration::from_secs_f64(audio.len() as f64 / SAMPLE_RATE as f64);
    if finals.is_empty() && duration >= FALLBACK_MIN_AUDIO {
        let result = pipeline.decode_all(audio)?;
        let lang = pipeline.router_mut().label(&result.text, duration);
        langs.push((duration, lang));
        finals.push(result.text);
    }
    let lang = langs.iter().max_by_key(|(d, _)| *d).map(|(_, l)| *l).unwrap_or(Lang::Sv);
    Ok((join(finals.iter().map(String::as_str)), lang))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tyst_core::asr::{AsrEngine, AsrResult};
    use tyst_core::pipeline::{EngineSet, PipelineConfig};
    use tyst_core::router::FixedRouter;
    use tyst_core::segmenter::EnergyDetector;
    use tyst_core::transcript::Channel;
    use tyst_core::vocabulary::VocabularyRules;
    use tyst_platform::AudioSource;

    use super::*;

    /// Says how much audio it got and which engine it is, so tests see what was decoded.
    struct FakeEngine(&'static str);

    impl AsrEngine for FakeEngine {
        fn id(&self) -> &str {
            self.0
        }
        fn transcribe(&mut self, pcm: &[f32]) -> tyst_core::Result<AsrResult> {
            Ok(AsrResult { text: format!(" {} {:.1}s ", self.0, pcm.len() as f32 / 16_000.0), ..Default::default() })
        }
    }

    /// Plays (seconds, loud?) pieces at 48 kHz in 20 ms chunks, `speed` times real time, then
    /// keeps the stream open (silent) like a microphone until stopped.
    struct FakeMic {
        pattern: Vec<(f32, f32)>,
        speed: f64,
        played: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl AudioSource for FakeMic {
        fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
            let (pattern, speed, played, stop) =
                (self.pattern.clone(), self.speed, self.played.clone(), self.stop.clone());
            self.thread = Some(std::thread::spawn(move || {
                let t0 = Instant::now();
                let mut n = 0usize;
                for (secs, amp) in pattern {
                    for _ in 0..(secs * 50.0) as usize {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        let samples: Vec<f32> = (0..960).map(|i| ((n + i) as f32 * 0.0167).sin() * amp).collect();
                        n += 960;
                        let due = t0 + Duration::from_secs_f64(n as f64 / 48_000.0 / speed);
                        if let Some(d) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(d);
                        }
                        if sink
                            .send(AudioChunk {
                                channel: Channel::Me,
                                sample_rate: 48_000,
                                samples,
                                captured_at: Instant::now(),
                            })
                            .is_err()
                        {
                            return;
                        }
                        played.store(n, Ordering::SeqCst);
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
            "fake mic".into()
        }
    }

    fn pipeline(mode: LanguageMode) -> ChannelPipeline<EnergyDetector> {
        let engines =
            EngineSet::new().with_primary(Box::new(FakeEngine("sv"))).with_english(Box::new(FakeEngine("en"))).shared();
        ChannelPipeline::new(
            Channel::Me,
            PipelineConfig::default(),
            EnergyDetector { window: 512, full_scale_rms: 0.1 },
            engines,
            Box::new(FixedRouter::new(mode, Lang::Sv, Duration::from_millis(1500))),
            Arc::new(VocabularyRules::default()),
        )
    }

    fn start(pattern: Vec<(f32, f32)>, speed: f64) -> (Dictation, Receiver<DictationEvent>, Arc<AtomicUsize>) {
        let played = Arc::new(AtomicUsize::new(0));
        let p2 = played.clone();
        let source: SourceFactory = Box::new(move || {
            Ok(Box::new(FakeMic { pattern, speed, played: p2, stop: Arc::default(), thread: None }) as _)
        });
        let (tx, rx) = mpsc::channel();
        let opts = DictationOptions { mode: LanguageMode::Auto, initial: Lang::Sv };
        let d = Dictation::start(pipeline(LanguageMode::Auto), source, opts, tx).unwrap();
        (d, rx, played)
    }

    fn wait_played(played: &AtomicUsize, secs: f64) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while (played.load(Ordering::SeqCst) as f64) < secs * 48_000.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn stopping_mid_speech_decodes_finished_and_open_segments() {
        // Two phrases with a pause between them, stopped while the second is still going.
        let (d, rx, played) = start(vec![(0.3, 0.001), (1.5, 0.3), (1.0, 0.001), (2.0, 0.3), (5.0, 0.3)], 4.0);
        wait_played(&played, 0.3 + 1.5 + 1.0 + 2.0);
        let out = d.finish().unwrap();
        assert_eq!(out.segments, 2, "{out:?}");
        let words: Vec<&str> = out.text.split(' ').collect();
        assert_eq!(words[0], "sv");
        assert_eq!(words[2], "sv");
        assert!(!out.text.starts_with(' ') && !out.text.ends_with(' ') && !out.text.contains("  "));
        assert!(out.duration >= Duration::from_secs(4), "{:?}", out.duration);
        assert_eq!(out.audio.len() as u64, (out.duration.as_secs_f64() * 16_000.0).round() as u64);
        let events: Vec<DictationEvent> = rx.try_iter().collect();
        assert!(
            events.iter().any(|e| matches!(e, DictationEvent::Text { committed, .. } if committed.starts_with("sv")))
        );
        assert!(events.iter().any(|e| matches!(e, DictationEvent::Text { partial, .. } if !partial.is_empty())));
        assert!(events.iter().any(|e| matches!(e, DictationEvent::Level { rms } if *rms > 0.1)));
    }

    #[test]
    fn language_switch_applies_to_the_next_segment() {
        let (d, _rx, played) = start(vec![(0.3, 0.001), (1.0, 0.3), (1.0, 0.001), (1.0, 0.3), (1.0, 0.001)], 4.0);
        d.set_language(LanguageMode::English);
        assert_eq!(d.language(), LanguageMode::English);
        wait_played(&played, 4.3);
        let out = d.finish().unwrap();
        assert!(out.text.starts_with("en"), "{}", out.text);
        assert_eq!(out.lang, Lang::En);
    }

    #[test]
    fn short_speech_the_vad_misses_is_decoded_whole() {
        // 0.2 s of speech is under the 0.25 s the VAD needs to open a segment.
        let (d, _rx, played) = start(vec![(0.2, 0.001), (0.2, 0.3), (0.2, 0.001)], 4.0);
        wait_played(&played, 0.6);
        let out = d.finish().unwrap();
        assert_eq!(out.segments, 1);
        assert!(out.text.starts_with("sv 0."), "{}", out.text);
    }

    #[test]
    fn silence_gives_no_text() {
        let (d, _rx, played) = start(vec![(1.0, 0.001)], 4.0);
        wait_played(&played, 1.0);
        let out = d.finish().unwrap();
        assert_eq!(out.text, "");
        assert_eq!(out.segments, 0);
    }

    #[test]
    fn cancel_discards() {
        let (d, _rx, played) = start(vec![(0.3, 0.001), (3.0, 0.3)], 4.0);
        wait_played(&played, 1.0);
        d.cancel();
    }

    #[test]
    fn retranscribes_kept_audio_in_another_language() {
        let (d, _rx, played) = start(vec![(0.3, 0.001), (1.0, 0.3), (0.8, 0.001)], 4.0);
        wait_played(&played, 2.1);
        let out = d.finish().unwrap();
        assert!(out.text.starts_with("sv"));
        let (text, lang) = retranscribe(pipeline(LanguageMode::English), &out.audio).unwrap();
        assert!(text.starts_with("en"), "{text}");
        assert_eq!(lang, Lang::En);
    }
}
