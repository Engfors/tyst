//! Per-channel pipeline (SPEC 6.1-6.2): 16 kHz audio -> VAD segmentation -> router -> ASR ->
//! vocabulary -> partial and final events.
//!
//! Live text is pseudo-streaming: while a segment is open, the growing buffer is re-decoded on a
//! schedule that backs off as the segment grows (ADR 0004: next partial after `max(0.8 s, L/2)`
//! more audio, for buffer length `L`), and the segment is decoded once more when it ends.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::asr::{AsrEngine, AsrResult};
use crate::router::{EngineRole, LanguageRouter};
use crate::segmenter::{Segmenter, SegmenterConfig, SegmenterEvent, SpeechDetector};
use crate::transcript::{Channel, SegState, Segment};
use crate::vocabulary::VocabularyRules;
use crate::{Error, Result};

type EngineLoader = Box<dyn FnMut() -> Result<Box<dyn AsrEngine>> + Send>;

/// An engine, loaded up front or on first use.
#[derive(Default)]
struct Slot {
    engine: Option<Box<dyn AsrEngine>>,
    loader: Option<EngineLoader>,
}

impl Slot {
    fn get(&mut self, what: &str) -> Result<&mut dyn AsrEngine> {
        if self.engine.is_none() {
            let loader = self.loader.as_mut().ok_or_else(|| Error::Model(format!("no {what} engine configured")))?;
            let t0 = Instant::now();
            let engine = loader()?;
            log::info!("loaded {what} engine {} in {:.2}s", engine.id(), t0.elapsed().as_secs_f32());
            self.engine = Some(engine);
        }
        Ok(self.engine.as_mut().expect("loaded above").as_mut())
    }
}

/// The ASR engines, shared by the channel pipelines of a session. Pianissimo is the primary
/// engine; Parakeet v3 is only needed when English is forced (ADR 0002), so either can be lazy.
#[derive(Default)]
pub struct EngineSet {
    primary: Slot,
    english: Slot,
}

impl EngineSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_primary(mut self, engine: Box<dyn AsrEngine>) -> Self {
        self.primary.engine = Some(engine);
        self
    }

    pub fn with_primary_loader(mut self, loader: EngineLoader) -> Self {
        self.primary.loader = Some(loader);
        self
    }

    pub fn with_english(mut self, engine: Box<dyn AsrEngine>) -> Self {
        self.english.engine = Some(engine);
        self
    }

    pub fn with_english_loader(mut self, loader: EngineLoader) -> Self {
        self.english.loader = Some(loader);
        self
    }

    /// Loads an engine now instead of on first use.
    pub fn preload(&mut self, role: EngineRole) -> Result<()> {
        self.engine(role).map(|_| ())
    }

    pub fn shared(self) -> SharedEngines {
        Arc::new(Mutex::new(self))
    }

    fn engine(&mut self, role: EngineRole) -> Result<&mut dyn AsrEngine> {
        match role {
            EngineRole::Primary => self.primary.get("primary"),
            EngineRole::English => self.english.get("English"),
        }
    }

    /// Ids of the engines loaded so far.
    pub fn loaded_ids(&self) -> Vec<String> {
        [&self.primary, &self.english].iter().filter_map(|s| s.engine.as_ref().map(|e| e.id().to_string())).collect()
    }
}

pub type SharedEngines = Arc<Mutex<EngineSet>>;

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub segmenter: SegmenterConfig,
    /// Emit partial results while a segment is open (live use). Off for file transcription.
    pub partials: bool,
    /// Shortest gap between partial decodes, in audio time.
    pub min_partial_interval: Duration,
    /// The gap also grows to this fraction of the buffer length.
    pub partial_growth: f32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            segmenter: SegmenterConfig::default(),
            partials: true,
            min_partial_interval: Duration::from_millis(800),
            partial_growth: 0.5,
        }
    }
}

/// Timing of one decode, for latency and CPU measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodeStats {
    /// Audio decoded.
    pub audio: Duration,
    /// Wall time of the decode.
    pub elapsed: Duration,
    /// Stream position (samples) of the newest audio in the decode.
    pub audio_end_sample: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    /// Replaces any earlier partial of the same segment.
    Partial {
        channel: Channel,
        segment_id: u64,
        start: Duration,
        end: Duration,
        text: String,
        stats: DecodeStats,
    },
    Final {
        segment: Segment,
        stats: DecodeStats,
    },
    /// The open segment turned out too short; drop its partial.
    Dropped {
        channel: Channel,
        segment_id: u64,
    },
}

pub struct ChannelPipeline<D: SpeechDetector> {
    channel: Channel,
    cfg: PipelineConfig,
    segmenter: Segmenter<D>,
    engines: SharedEngines,
    router: Box<dyn LanguageRouter>,
    vocabulary: Arc<VocabularyRules>,
    next_id: u64,
    open_id: Option<u64>,
    /// Buffer length (samples) at which the next partial is due.
    next_partial_at: u64,
    sample_rate: u32,
}

impl<D: SpeechDetector> ChannelPipeline<D> {
    pub fn new(
        channel: Channel,
        cfg: PipelineConfig,
        detector: D,
        engines: SharedEngines,
        router: Box<dyn LanguageRouter>,
        vocabulary: Arc<VocabularyRules>,
    ) -> Self {
        let sample_rate = cfg.segmenter.sample_rate;
        let first = (cfg.min_partial_interval.as_secs_f32() * sample_rate as f32) as u64;
        Self {
            channel,
            segmenter: Segmenter::new(cfg.segmenter.clone(), detector),
            cfg,
            engines,
            router,
            vocabulary,
            next_id: 1,
            open_id: None,
            next_partial_at: first,
            sample_rate,
        }
    }

    pub fn router_mut(&mut self) -> &mut dyn LanguageRouter {
        self.router.as_mut()
    }

    /// Samples consumed so far.
    pub fn position(&self) -> u64 {
        self.segmenter.position()
    }

    /// Feeds 16 kHz mono audio.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<PipelineEvent>> {
        let seg_events = self.segmenter.push(samples)?;
        let mut out = Vec::new();
        for e in seg_events {
            self.handle(e, &mut out)?;
        }
        if self.cfg.partials {
            self.maybe_partial(&mut out)?;
        }
        Ok(out)
    }

    /// Ends the stream and finalizes an open segment.
    pub fn flush(&mut self) -> Result<Vec<PipelineEvent>> {
        let mut out = Vec::new();
        for e in self.segmenter.flush() {
            self.handle(e, &mut out)?;
        }
        Ok(out)
    }

    fn handle(&mut self, event: SegmenterEvent, out: &mut Vec<PipelineEvent>) -> Result<()> {
        match event {
            SegmenterEvent::Started { .. } => {
                self.open_id = Some(self.next_id);
                self.next_id += 1;
                self.next_partial_at = self.samples(self.cfg.min_partial_interval);
            }
            SegmenterEvent::Dropped { .. } => {
                if let Some(id) = self.open_id.take() {
                    out.push(PipelineEvent::Dropped { channel: self.channel, segment_id: id });
                }
            }
            SegmenterEvent::Ended(seg) => {
                let id = self.open_id.take().unwrap_or_else(|| {
                    self.next_id += 1;
                    self.next_id - 1
                });
                let role = self.router.engine();
                let (result, engine_id, stats) = self.decode(role, &seg.samples, seg.end())?;
                let start = self.duration(seg.start);
                let end = self.duration(seg.end());
                let text = self.vocabulary.apply(&result.text);
                let lang = self.router.label(&text, end - start);
                out.push(PipelineEvent::Final {
                    segment: Segment {
                        id,
                        channel: self.channel,
                        start,
                        end,
                        lang,
                        engine: engine_id,
                        text,
                        state: SegState::Final,
                        edited: false,
                    },
                    stats,
                });
            }
        }
        Ok(())
    }

    fn maybe_partial(&mut self, out: &mut Vec<PipelineEvent>) -> Result<()> {
        let (Some(id), Some((start, samples))) = (self.open_id, self.segmenter.active()) else {
            return Ok(());
        };
        let len = samples.len() as u64;
        if len < self.next_partial_at {
            return Ok(());
        }
        let samples = samples.to_vec();
        let end_sample = start + len;
        let role = self.router.engine();
        let (result, _, stats) = self.decode(role, &samples, end_sample)?;
        let min = self.samples(self.cfg.min_partial_interval);
        let growth = (len as f32 * self.cfg.partial_growth) as u64;
        self.next_partial_at = len + min.max(growth);
        out.push(PipelineEvent::Partial {
            channel: self.channel,
            segment_id: id,
            start: self.duration(start),
            end: self.duration(end_sample),
            text: self.vocabulary.apply(&result.text),
            stats,
        });
        Ok(())
    }

    fn decode(
        &mut self,
        role: EngineRole,
        samples: &[f32],
        end_sample: u64,
    ) -> Result<(AsrResult, String, DecodeStats)> {
        let mut engines = self.engines.lock().map_err(|_| Error::Model("engine lock poisoned".into()))?;
        let engine = engines.engine(role)?;
        let t0 = Instant::now();
        let result = engine.transcribe(samples)?;
        let stats = DecodeStats {
            audio: Duration::from_secs_f64(samples.len() as f64 / self.sample_rate as f64),
            elapsed: t0.elapsed(),
            audio_end_sample: end_sample,
        };
        Ok((result, engine.id().to_string(), stats))
    }

    fn samples(&self, d: Duration) -> u64 {
        (d.as_secs_f64() * self.sample_rate as f64) as u64
    }

    fn duration(&self, samples: u64) -> Duration {
        Duration::from_secs_f64(samples as f64 / self.sample_rate as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{FixedRouter, LanguageMode};
    use crate::segmenter::EnergyDetector;
    use crate::transcript::Lang;
    use crate::vocabulary::{Replacement, VocabularyFile};

    /// Reports how much audio it saw, so tests can check what was decoded.
    struct FakeEngine {
        id: String,
        calls: Arc<Mutex<Vec<usize>>>,
    }

    impl AsrEngine for FakeEngine {
        fn id(&self) -> &str {
            &self.id
        }
        fn transcribe(&mut self, pcm: &[f32]) -> Result<AsrResult> {
            self.calls.lock().unwrap().push(pcm.len());
            Ok(AsrResult {
                text: format!("vi kör terra form {} samples", pcm.len()), tokens: vec![], confidence: -0.1
            })
        }
    }

    fn tone(secs: f32, loud: bool) -> Vec<f32> {
        (0..(secs * 16_000.0) as usize).map(|i| (i as f32 * 0.05).sin() * if loud { 0.3 } else { 0.001 }).collect()
    }

    fn pipeline(partials: bool, calls: Arc<Mutex<Vec<usize>>>) -> ChannelPipeline<EnergyDetector> {
        let engines = EngineSet::new()
            .with_primary(Box::new(FakeEngine { id: "sv@1".into(), calls: calls.clone() }))
            .with_english_loader(Box::new(move || Ok(Box::new(FakeEngine { id: "en@1".into(), calls: calls.clone() }))))
            .shared();
        let vocab = VocabularyRules::new(&VocabularyFile {
            terms: vec![],
            replacements: vec![Replacement { from: "terra form".into(), to: "Terraform".into() }],
        });
        ChannelPipeline::new(
            Channel::Me,
            PipelineConfig { partials, ..Default::default() },
            EnergyDetector { window: 512, full_scale_rms: 0.1 },
            engines,
            Box::new(FixedRouter::default()),
            Arc::new(vocab),
        )
    }

    fn run(p: &mut ChannelPipeline<EnergyDetector>, pcm: &[f32]) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        for chunk in pcm.chunks(512) {
            events.extend(p.push(chunk).unwrap());
        }
        events.extend(p.flush().unwrap());
        events
    }

    #[test]
    fn emits_backed_off_partials_then_a_final() {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut p = pipeline(true, calls.clone());
        let pcm = [tone(0.5, false), tone(10.0, true), tone(1.0, false)].concat();
        let events = run(&mut p, &pcm);
        let partial_lens: Vec<f32> = events
            .iter()
            .filter_map(|e| match e {
                PipelineEvent::Partial { start, end, .. } => Some((*end - *start).as_secs_f32()),
                _ => None,
            })
            .collect();
        // ADR 0004 schedule: first after 0.8 s, then each gap is max(0.8 s, L/2) of the buffer
        // length L at the previous partial (0.8, 1.6, 2.4, 3.6, 5.4, 8.1 s for ideal timing).
        assert_eq!(partial_lens.len(), 6, "{partial_lens:?}");
        assert!((partial_lens[0] - 0.8).abs() < 0.05, "{partial_lens:?}");
        for w in partial_lens.windows(2) {
            let gap = w[1] - w[0];
            assert!((gap - (w[0] / 2.0).max(0.8)).abs() < 0.05, "{partial_lens:?}");
        }
        let finals: Vec<&Segment> = events
            .iter()
            .filter_map(|e| match e {
                PipelineEvent::Final { segment, .. } => Some(segment),
                _ => None,
            })
            .collect();
        assert_eq!(finals.len(), 1);
        let f = finals[0];
        assert_eq!(f.state, SegState::Final);
        assert!(f.text.starts_with("vi kör Terraform"), "vocabulary applied: {}", f.text);
        assert_eq!(f.lang, Lang::Sv);
        assert_eq!(f.engine, "sv@1");
        // Partials and the final share the segment id.
        assert!(events.iter().all(|e| match e {
            PipelineEvent::Partial { segment_id, .. } => *segment_id == f.id,
            _ => true,
        }));
        assert_eq!(calls.lock().unwrap().len(), partial_lens.len() + 1);
    }

    #[test]
    fn file_mode_decodes_each_segment_once() {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut p = pipeline(false, calls.clone());
        let pcm = [tone(0.5, false), tone(2.0, true), tone(1.0, false), tone(3.0, true), tone(1.0, false)].concat();
        let events = run(&mut p, &pcm);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| matches!(e, PipelineEvent::Final { .. })));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn forced_english_loads_the_english_engine_lazily() {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut p = pipeline(false, calls);
        p.router_mut().set_mode(LanguageMode::English);
        let events = run(&mut p, &[tone(0.5, false), tone(2.0, true), tone(1.0, false)].concat());
        match &events[0] {
            PipelineEvent::Final { segment, .. } => {
                assert_eq!(segment.engine, "en@1");
                assert_eq!(segment.lang, Lang::En);
            }
            e => panic!("unexpected {e:?}"),
        }
    }
}
