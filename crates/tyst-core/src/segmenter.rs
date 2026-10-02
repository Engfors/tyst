//! Voice-activity segmentation (SPEC 6.2, ADR 0004).
//!
//! The speech/silence state machine follows sherpa-onnx's Silero VAD wrapper, which produced the
//! Phase 0 segments: a window counts as speech above `threshold`, stays speech above
//! `threshold - 0.15` once triggered, and speech ends after `min_silence` below the threshold.
//! The 15 s force-cut follows SPEC 6.2: cut at the quietest point in the last 2 s and continue in
//! a new segment.

use crate::Result;

/// Something that scores fixed-size windows of 16 kHz audio with a speech probability.
pub trait SpeechDetector: Send {
    /// Samples per call to [`prob`](Self::prob).
    fn window_size(&self) -> usize;
    fn prob(&mut self, window: &[f32]) -> Result<f32>;
    /// Clears recurrent state, e.g. between files.
    fn reset(&mut self);
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmenterConfig {
    pub sample_rate: u32,
    pub threshold: f32,
    /// Silence that ends a segment.
    pub min_silence: f32,
    /// Speech needed before a segment starts.
    pub min_speech: f32,
    /// Force-cut length.
    pub max_speech: f32,
    /// Where a force-cut may land, counted back from `max_speech`.
    pub cut_search: f32,
    /// Finished segments shorter than this are dropped.
    pub drop_shorter_than: f32,
}

impl Default for SegmenterConfig {
    /// ADR 0004 values.
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            threshold: 0.5,
            min_silence: 0.5,
            min_speech: 0.25,
            max_speech: 15.0,
            cut_search: 2.0,
            drop_shorter_than: 0.3,
        }
    }
}

/// A finished stretch of speech.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSegment {
    /// Index of the first sample in the channel's stream.
    pub start: u64,
    pub samples: Vec<f32>,
    /// Ended by the force-cut rather than by silence.
    pub forced: bool,
}

impl SpeechSegment {
    pub fn end(&self) -> u64 {
        self.start + self.samples.len() as u64
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegmenterEvent {
    /// Speech started at this sample index.
    Started { start: u64 },
    /// Speech ended (by silence, force-cut, or end of input).
    Ended(SpeechSegment),
    /// An open segment ended too short to keep.
    Dropped { start: u64, end: u64 },
}

pub struct Segmenter<D: SpeechDetector> {
    cfg: SegmenterConfig,
    detector: D,
    window: usize,
    /// Samples waiting for a full window.
    pending: Vec<f32>,
    /// Audio since `buffer_start`: the open segment, or pre-roll while idle.
    buffer: Vec<f32>,
    buffer_start: u64,
    /// Samples run through the detector so far.
    current: u64,
    triggered: bool,
    temp_start: u64,
    temp_end: u64,
    /// Start of the open segment.
    start: Option<u64>,
    last_prob: f32,
}

impl<D: SpeechDetector> Segmenter<D> {
    pub fn new(cfg: SegmenterConfig, detector: D) -> Self {
        let window = detector.window_size();
        Self {
            cfg,
            detector,
            window,
            pending: Vec::new(),
            buffer: Vec::new(),
            buffer_start: 0,
            current: 0,
            triggered: false,
            temp_start: 0,
            temp_end: 0,
            start: None,
            last_prob: 0.0,
        }
    }

    pub fn config(&self) -> &SegmenterConfig {
        &self.cfg
    }

    /// Feeds audio and returns what happened in it.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<SegmenterEvent>> {
        let mut events = Vec::new();
        self.pending.extend_from_slice(samples);
        let mut offset = 0;
        while self.pending.len() - offset >= self.window {
            let end = offset + self.window;
            let window: Vec<f32> = self.pending[offset..end].to_vec();
            offset = end;
            self.process_window(&window, &mut events)?;
        }
        self.pending.drain(..offset);
        Ok(events)
    }

    /// Ends the input: closes an open segment with everything buffered so far.
    pub fn flush(&mut self) -> Vec<SegmenterEvent> {
        let mut events = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        self.buffer.extend_from_slice(&pending);
        if let Some(start) = self.start.take() {
            let tail = self.tail();
            self.emit(start, tail, false, &mut events);
        }
        self.reset_state();
        events
    }

    /// The open segment so far (start index and samples), for partial decodes.
    pub fn active(&self) -> Option<(u64, &[f32])> {
        self.start.map(|s| (s, &self.buffer[(s - self.buffer_start) as usize..]))
    }

    /// Speech probability of the latest window (for level meters and debugging).
    pub fn last_prob(&self) -> f32 {
        self.last_prob
    }

    /// Samples consumed (the stream position of the segmenter).
    pub fn position(&self) -> u64 {
        self.current
    }

    fn reset_state(&mut self) {
        self.triggered = false;
        self.temp_start = 0;
        self.temp_end = 0;
        self.start = None;
        self.detector.reset();
        self.buffer_start += self.buffer.len() as u64;
        self.buffer.clear();
    }

    fn tail(&self) -> u64 {
        self.buffer_start + self.buffer.len() as u64
    }

    fn secs(&self, s: f32) -> u64 {
        (s * self.cfg.sample_rate as f32).round() as u64
    }

    fn process_window(&mut self, window: &[f32], events: &mut Vec<SegmenterEvent>) -> Result<()> {
        self.buffer.extend_from_slice(window);
        let prob = self.detector.prob(window)?;
        self.last_prob = prob;
        self.current += window.len() as u64;
        let is_speech = self.is_speech(prob);
        let pre_roll = 2 * self.window as u64 + self.secs(self.cfg.min_speech);

        if is_speech {
            if self.start.is_none() {
                let start = self.tail().saturating_sub(pre_roll).max(self.buffer_start);
                self.start = Some(start);
                events.push(SegmenterEvent::Started { start });
            }
            self.force_cut_if_needed(events);
        } else {
            if let Some(start) = self.start.take() {
                let end = self.tail().saturating_sub(self.secs(self.cfg.min_silence)).max(start);
                self.emit(start, end, false, events);
            }
            // Idle: keep only the pre-roll.
            let keep_from = self.tail().saturating_sub(pre_roll).max(self.buffer_start);
            self.pop_until(keep_from);
        }
        Ok(())
    }

    /// sherpa-onnx `SileroVadModel::IsSpeech`.
    fn is_speech(&mut self, prob: f32) -> bool {
        let thr = self.cfg.threshold;
        let min_speech = self.secs(self.cfg.min_speech);
        let min_silence = self.secs(self.cfg.min_silence);
        let now = self.current;
        if prob > thr && self.temp_end != 0 {
            self.temp_end = 0;
        }
        if prob > thr && self.temp_start == 0 {
            self.temp_start = now;
            return false;
        }
        if prob > thr && self.temp_start != 0 && !self.triggered {
            if now - self.temp_start < min_speech {
                return false;
            }
            self.triggered = true;
            return true;
        }
        if prob < thr && !self.triggered {
            self.temp_start = 0;
            self.temp_end = 0;
            return false;
        }
        if prob > thr - 0.15 && self.triggered {
            return true;
        }
        if prob < thr && self.triggered {
            if self.temp_end == 0 {
                self.temp_end = now;
            }
            if now - self.temp_end < min_silence {
                return true;
            }
            self.temp_start = 0;
            self.temp_end = 0;
            self.triggered = false;
            return false;
        }
        false
    }

    fn force_cut_if_needed(&mut self, events: &mut Vec<SegmenterEvent>) {
        let Some(start) = self.start else { return };
        let max = self.secs(self.cfg.max_speech);
        if self.tail() - start < max {
            return;
        }
        let search = self.secs(self.cfg.cut_search).min(max);
        let from = self.tail() - search;
        let cut = self.quietest_point(from, self.tail());
        self.emit(start, cut, true, events);
        self.pop_until(cut);
        self.start = Some(cut);
        events.push(SegmenterEvent::Started { start: cut });
    }

    /// Centre of the 20 ms frame with the least energy in `[from, to)`.
    fn quietest_point(&self, from: u64, to: u64) -> u64 {
        let frame = (self.cfg.sample_rate / 50) as u64;
        let mut best = (f32::INFINITY, to);
        let mut pos = from;
        while pos + frame <= to {
            let a = (pos - self.buffer_start) as usize;
            let energy: f32 = self.buffer[a..a + frame as usize].iter().map(|x| x * x).sum();
            if energy < best.0 {
                best = (energy, pos + frame / 2);
            }
            pos += frame;
        }
        best.1
    }

    fn emit(&mut self, start: u64, end: u64, forced: bool, events: &mut Vec<SegmenterEvent>) {
        let min = self.secs(self.cfg.drop_shorter_than);
        if end <= start || end - start < min {
            events.push(SegmenterEvent::Dropped { start, end });
            return;
        }
        let a = (start - self.buffer_start) as usize;
        let b = (end - self.buffer_start) as usize;
        events.push(SegmenterEvent::Ended(SpeechSegment { start, samples: self.buffer[a..b].to_vec(), forced }));
    }

    fn pop_until(&mut self, index: u64) {
        if index > self.buffer_start {
            let n = ((index - self.buffer_start) as usize).min(self.buffer.len());
            self.buffer.drain(..n);
            self.buffer_start += n as u64;
        }
    }
}

/// A detector that calls loud windows speech; for tests and as a no-model fallback in tooling.
pub struct EnergyDetector {
    pub window: usize,
    /// RMS that maps to probability 1.
    pub full_scale_rms: f32,
}

impl SpeechDetector for EnergyDetector {
    fn window_size(&self) -> usize {
        self.window
    }
    fn prob(&mut self, window: &[f32]) -> Result<f32> {
        let rms = (window.iter().map(|x| x * x).sum::<f32>() / window.len() as f32).sqrt();
        Ok((rms / self.full_scale_rms).min(1.0))
    }
    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 16_000;

    fn detector() -> EnergyDetector {
        EnergyDetector { window: 512, full_scale_rms: 0.1 }
    }

    /// `pattern` of (seconds, loud?) pieces; loud is a 0.3-amplitude tone, quiet is near-silence.
    fn audio(pattern: &[(f32, bool)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(secs, loud) in pattern {
            let n = (secs * SR as f32) as usize;
            for i in 0..n {
                let tone = (i as f32 * 0.05).sin();
                out.push(if loud { 0.3 * tone } else { 0.001 * tone });
            }
        }
        out
    }

    fn run(cfg: SegmenterConfig, pcm: &[f32]) -> Vec<SegmenterEvent> {
        let mut seg = Segmenter::new(cfg, detector());
        let mut events = Vec::new();
        for chunk in pcm.chunks(1600) {
            events.extend(seg.push(chunk).unwrap());
        }
        events.extend(seg.flush());
        events
    }

    fn ended(events: &[SegmenterEvent]) -> Vec<&SpeechSegment> {
        events
            .iter()
            .filter_map(|e| match e {
                SegmenterEvent::Ended(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn secs(samples: u64) -> f32 {
        samples as f32 / SR as f32
    }

    #[test]
    fn splits_on_silence() {
        let pcm = audio(&[(1.0, false), (2.0, true), (1.0, false), (1.5, true), (1.0, false)]);
        let events = run(SegmenterConfig::default(), &pcm);
        let segs = ended(&events);
        assert_eq!(segs.len(), 2);
        // Starts with a little pre-roll before the speech onset.
        assert!((secs(segs[0].start) - 1.0).abs() < 0.1, "start {}", secs(segs[0].start));
        assert!((secs(segs[0].end()) - 3.0).abs() < 0.1, "end {}", secs(segs[0].end()));
        assert!((secs(segs[1].start) - 4.0).abs() < 0.1);
        assert!(segs.iter().all(|s| !s.forced));
    }

    #[test]
    fn short_pauses_do_not_split() {
        let pcm = audio(&[(0.5, false), (1.0, true), (0.3, false), (1.0, true), (1.0, false)]);
        let segs = run(SegmenterConfig::default(), &pcm);
        assert_eq!(ended(&segs).len(), 1);
    }

    #[test]
    fn blips_shorter_than_min_speech_are_ignored() {
        let pcm = audio(&[(1.0, false), (0.15, true), (1.0, false)]);
        let events = run(SegmenterConfig::default(), &pcm);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn drops_segments_shorter_than_threshold() {
        let cfg = SegmenterConfig { drop_shorter_than: 1.0, ..Default::default() };
        let pcm = audio(&[(1.0, false), (0.6, true), (1.0, false)]);
        let events = run(cfg, &pcm);
        assert!(ended(&events).is_empty());
        assert!(events.iter().any(|e| matches!(e, SegmenterEvent::Dropped { .. })));
    }

    #[test]
    fn force_cuts_long_speech_at_the_quietest_point() {
        // 20 s of speech with a short dip (too short to end the segment) at 13.5 s.
        let pcm = audio(&[(0.5, false), (13.0, true), (0.2, false), (7.0, true), (1.0, false)]);
        let events = run(SegmenterConfig::default(), &pcm);
        let segs = ended(&events);
        assert_eq!(segs.len(), 2, "{:?}", segs.iter().map(|s| (s.start, s.end())).collect::<Vec<_>>());
        assert!(segs[0].forced);
        let cut = secs(segs[0].end());
        assert!((13.4..13.8).contains(&cut), "cut at {cut}");
        assert!(secs(segs[0].samples.len() as u64) <= 15.0);
        // The next segment continues exactly where the cut was.
        assert_eq!(segs[1].start, segs[0].end());
        assert!(!segs[1].forced);
    }

    #[test]
    fn force_cut_without_a_dip_stays_within_max_speech() {
        let pcm = audio(&[(0.5, false), (40.0, true), (1.0, false)]);
        let segs = run(SegmenterConfig::default(), &pcm);
        let segs = ended(&segs);
        assert_eq!(segs.len(), 3);
        for s in &segs {
            assert!(secs(s.samples.len() as u64) <= 15.0 + 0.04);
        }
        // Contiguous coverage of the speech.
        assert_eq!(segs[1].start, segs[0].end());
        assert_eq!(segs[2].start, segs[1].end());
    }

    #[test]
    fn active_exposes_the_open_segment() {
        let mut seg = Segmenter::new(SegmenterConfig::default(), detector());
        seg.push(&audio(&[(0.5, false), (1.0, true)])).unwrap();
        let (start, samples) = seg.active().expect("segment open");
        assert!(secs(start) < 0.55);
        assert!(samples.len() as f32 / SR as f32 > 0.9);
    }

    #[test]
    fn flush_closes_open_segment() {
        let events = run(SegmenterConfig::default(), &audio(&[(0.5, false), (2.0, true)]));
        let segs = ended(&events);
        assert_eq!(segs.len(), 1);
        assert!((secs(segs[0].end()) - 2.5).abs() < 0.05);
    }
}
