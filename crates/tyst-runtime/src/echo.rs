//! Echo cancellation on the Me channel (SPEC 6.4, Phase 5). On speakers the microphone also hears
//! the other participants, whose words would show up twice: once as Others and once as Me. The
//! Others channel already has exactly that signal (the system audio), so it is the reference for
//! WebRTC's AEC3 running on the microphone.
//!
//! Both channels run on their own capture clocks. The Others worker writes its 16 kHz audio into
//! an [`EchoReference`] at its position on the session clock (wall-clock based, like segment
//! times); the Me worker's [`EchoCanceller`] reads the reference at the same positions, ten
//! milliseconds at a time. AEC3 finds the remaining delay itself (the acoustic path plus device
//! latencies). When system audio is flowing, Me waits briefly for the reference to catch up, so
//! a slow Others thread does not make the canceller miss the echo.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tyst_core::resample::SAMPLE_RATE;

/// One AEC frame: 10 ms at 16 kHz.
pub const FRAME: usize = SAMPLE_RATE as usize / 100;
/// Positions closer than this (50 ms) count as continuous; capture callbacks jitter by a few ms.
const TOLERANCE: u64 = SAMPLE_RATE as u64 / 20;
/// Reference audio kept for late readers.
const KEEP: usize = SAMPLE_RATE as usize * 3;
/// Others counts as playing while it delivered audio this recently.
const ACTIVE: Duration = Duration::from_millis(300);
/// The longest Me waits for the reference (in samples behind the session clock): 250 ms.
const MAX_WAIT: u64 = SAMPLE_RATE as u64 / 4;

/// System audio as the echo reference, written by the Others worker and read by Me.
#[derive(Default)]
pub struct EchoReference {
    ring: Mutex<Ring>,
}

#[derive(Default)]
struct Ring {
    /// Session position of `buf[0]`.
    start: u64,
    buf: VecDeque<f32>,
    last_push: Option<Instant>,
}

impl Ring {
    fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }
}

impl EchoReference {
    /// Appends system audio that starts at session position `at`. Small timing jitter is ignored
    /// (the audio is appended where the last chunk ended); a gap is filled with silence and
    /// audio the reference already has is skipped, so it stays within 50 ms of the session clock.
    pub fn push(&self, at: u64, pcm: &[f32]) {
        let mut r = self.ring.lock().expect("echo reference lock");
        let end = r.end();
        let mut pcm = pcm;
        if r.buf.is_empty() {
            r.start = at;
        } else if at > end + TOLERANCE {
            let gap = (at - end) as usize;
            if gap >= KEEP {
                r.buf.clear();
                r.start = at;
            } else {
                r.buf.extend(std::iter::repeat_n(0.0, gap));
            }
        } else if at + TOLERANCE < end {
            let skip = ((end - at) as usize).min(pcm.len());
            pcm = &pcm[skip..];
        }
        r.buf.extend(pcm.iter().copied());
        if r.buf.len() > KEEP {
            let extra = r.buf.len() - KEEP;
            r.buf.drain(..extra);
            r.start += extra as u64;
        }
        r.last_push = Some(Instant::now());
    }

    /// Fills `out` with the reference from session position `pos`, silence where there is none.
    pub fn read(&self, pos: u64, out: &mut [f32]) {
        let r = self.ring.lock().expect("echo reference lock");
        for (i, x) in out.iter_mut().enumerate() {
            let p = pos + i as u64;
            *x = if p >= r.start && p < r.end() { r.buf[(p - r.start) as usize] } else { 0.0 };
        }
    }

    /// Whether the reference up to `until` is there, or will not come because nothing plays.
    fn ready(&self, until: u64) -> bool {
        let r = self.ring.lock().expect("echo reference lock");
        r.end() >= until || r.last_push.is_none_or(|t| t.elapsed() > ACTIVE)
    }
}

fn energy(x: &[f32]) -> f32 {
    x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32
}

/// Audio of the Me channel after echo cancellation, at its session position.
pub type Block = (u64, Vec<f32>);

/// AEC3 on the Me channel, reading the system audio from an [`EchoReference`].
pub struct EchoCanceller {
    #[cfg(feature = "aec")]
    ap: webrtc_audio_processing::Processor,
    reference: std::sync::Arc<EchoReference>,
    /// Session position of `pending[0]`.
    pos: u64,
    pending: VecDeque<f32>,
    render: Vec<f32>,
    failed: bool,
    /// Residual echo gate: smoothed frame energies before and after AEC3, and the gain applied.
    energy_in: f32,
    energy_out: f32,
    gain: f32,
}

/// Frames where AEC3 removed more than this share of the energy (−12 dB) are echo, not the user
/// talking over the others; what is left of them is turned down further, so faint residual echo
/// does not reach the recognizer as stray words.
const ECHO_RATIO: f32 = 0.063;
/// Gain on residual echo (−18 dB).
const RESIDUAL_GAIN: f32 = 0.125;
/// Below this frame energy (about −60 dBFS) the gate leaves the microphone alone.
const SILENCE: f32 = 1e-6;

impl EchoCanceller {
    #[cfg(feature = "aec")]
    pub fn new(reference: std::sync::Arc<EchoReference>) -> Result<Self, String> {
        use webrtc_audio_processing_config::{Config, EchoCanceller as Aec};
        let ap = webrtc_audio_processing::Processor::new(SAMPLE_RATE).map_err(|e| e.to_string())?;
        // Echo cancellation only: noise suppression and gain control change the speech the
        // recognizer was trained on.
        ap.set_config(Config { echo_canceller: Some(Aec::default()), ..Default::default() });
        debug_assert_eq!(ap.num_samples_per_frame(), FRAME);
        Ok(Self {
            ap,
            reference,
            pos: 0,
            pending: VecDeque::new(),
            render: vec![0.0; FRAME],
            failed: false,
            energy_in: 0.0,
            energy_out: 0.0,
            gain: 1.0,
        })
    }

    #[cfg(not(feature = "aec"))]
    pub fn new(_reference: std::sync::Arc<EchoReference>) -> Result<Self, String> {
        Err("this build has no echo cancellation".into())
    }

    /// Takes Me audio that starts at session position `at`; returns what is ready (cancelled up
    /// to where the reference has arrived, or `now` is far enough along not to wait for it).
    pub fn push(&mut self, at: u64, pcm: &[f32], now: u64) -> Vec<Block> {
        let mut out = Vec::new();
        if self.pending.is_empty() {
            self.pos = at;
        } else if at.abs_diff(self.pos + self.pending.len() as u64) > TOLERANCE {
            // A capture gap (or a jump of the clock): finish what is pending first.
            out.extend(self.drain(u64::MAX, true));
            self.pos = at;
        }
        self.pending.extend(pcm.iter().copied());
        out.extend(self.drain(now, false));
        out
    }

    /// What became ready without new Me audio (the reference arrived, or time passed).
    pub fn poll(&mut self, now: u64) -> Vec<Block> {
        self.drain(now, false)
    }

    /// Everything still pending, without waiting.
    pub fn flush(&mut self) -> Vec<Block> {
        self.drain(u64::MAX, true)
    }

    fn drain(&mut self, now: u64, all: bool) -> Vec<Block> {
        let start = self.pos;
        let mut block = Vec::new();
        while self.pending.len() >= FRAME {
            let end = self.pos + FRAME as u64;
            if !all && !self.reference.ready(end) && now < end + MAX_WAIT {
                break;
            }
            let mut frame: Vec<f32> = self.pending.drain(..FRAME).collect();
            self.reference.read(self.pos, &mut self.render);
            self.cancel(&mut frame);
            block.extend_from_slice(&frame);
            self.pos = end;
        }
        if all && !self.pending.is_empty() {
            // Less than a frame left: passes through as it is.
            block.extend(self.pending.drain(..));
            self.pos = start + block.len() as u64;
        }
        if block.is_empty() { Vec::new() } else { vec![(start, block)] }
    }

    #[cfg(feature = "aec")]
    fn cancel(&mut self, frame: &mut Vec<f32>) {
        if self.failed {
            return;
        }
        let before = energy(frame);
        let r = self
            .ap
            .analyze_render_frame(std::slice::from_ref(&self.render))
            .and_then(|()| self.ap.process_capture_frame(std::slice::from_mut(frame)));
        if let Err(e) = r {
            log::error!("echo cancellation failed, Me continues without it: {e}");
            self.failed = true;
            return;
        }
        self.energy_in = 0.7 * self.energy_in + 0.3 * before;
        self.energy_out = 0.7 * self.energy_out + 0.3 * energy(frame);
        let echo = self.energy_in > SILENCE && self.energy_out < self.energy_in * ECHO_RATIO;
        let target = if echo { RESIDUAL_GAIN } else { 1.0 };
        // Down quickly when echo starts, back up quickly when the user speaks (10 ms frames).
        for x in frame.iter_mut() {
            self.gain += (target - self.gain) * 0.02;
            *x *= self.gain;
        }
    }

    #[cfg(not(feature = "aec"))]
    fn cancel(&mut self, _frame: &mut Vec<f32>) {}

    /// The delay AEC3 currently estimates between system audio and its echo, if it found one.
    #[cfg(feature = "aec")]
    pub fn delay_ms(&self) -> Option<u32> {
        self.ap.get_stats().delay_ms
    }

    #[cfg(not(feature = "aec"))]
    pub fn delay_ms(&self) -> Option<u32> {
        None
    }
}

#[cfg(all(test, feature = "aec"))]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Speech-like noise: a pseudo-random signal with a syllable-rate envelope.
    fn voice(seed: u32, n: usize) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|i| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                let env = 0.5 + 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 4.0 / 16_000.0).sin();
                noise * env * 0.5
            })
            .collect()
    }

    fn power(x: &[f32]) -> f64 {
        x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len().max(1) as f64
    }

    #[test]
    fn removes_the_system_audio_from_the_microphone() {
        let reference = Arc::new(EchoReference::default());
        let mut aec = EchoCanceller::new(reference.clone()).unwrap();
        let secs = 8;
        let far = voice(1, 16_000 * secs);
        // The microphone hears the speakers 60 ms later, at half the level, plus a little noise.
        let delay = 960;
        let noise = voice(7, far.len());
        let mic: Vec<f32> =
            (0..far.len()).map(|i| if i >= delay { far[i - delay] * 0.5 } else { 0.0 } + noise[i] * 0.01).collect();
        let mut out = Vec::new();
        for (i, (f, m)) in far.chunks(320).zip(mic.chunks(320)).enumerate() {
            let at = (i * 320) as u64;
            reference.push(at, f);
            for (p, b) in aec.push(at, m, at + 320) {
                assert_eq!(p as usize, out.len());
                out.extend(b);
            }
        }
        for (_, b) in aec.flush() {
            out.extend(b);
        }
        assert_eq!(out.len(), mic.len());
        // After AEC3 has converged (the last 3 s), the echo is at least 20 dB down.
        let tail = 16_000 * (secs - 3);
        let erle = 10.0 * (power(&mic[tail..]) / power(&out[tail..])).log10();
        assert!(erle > 20.0, "echo only {erle:.1} dB down");
        assert!(aec.delay_ms().is_some());
    }

    #[test]
    fn keeps_the_user_talking_over_the_others() {
        let reference = Arc::new(EchoReference::default());
        let mut aec = EchoCanceller::new(reference.clone()).unwrap();
        let n = 16_000 * 8;
        let far = voice(1, n);
        let near: Vec<f32> =
            voice(5, n).iter().enumerate().map(|(i, &v)| if i >= 16_000 * 5 { v * 0.5 } else { 0.0 }).collect();
        let delay = 960;
        let mic: Vec<f32> = (0..n).map(|i| if i >= delay { far[i - delay] * 0.5 } else { 0.0 } + near[i]).collect();
        let mut out = Vec::new();
        for (i, (f, m)) in far.chunks(320).zip(mic.chunks(320)).enumerate() {
            let at = (i * 320) as u64;
            reference.push(at, f);
            for (_, b) in aec.push(at, m, at + 320) {
                out.extend(b);
            }
        }
        for (_, b) in aec.flush() {
            out.extend(b);
        }
        // Double talk (5-8 s): the user's speech comes through within 6 dB.
        let dt = 16_000 * 5 + 1600;
        let kept = 10.0 * (power(&out[dt..]) / power(&near[dt..])).log10();
        assert!(kept > -6.0, "near-end speech {kept:.1} dB");
    }

    #[test]
    fn keeps_near_end_speech_when_nothing_plays() {
        let reference = Arc::new(EchoReference::default());
        let mut aec = EchoCanceller::new(reference).unwrap();
        let me = voice(3, 16_000 * 3);
        let mut out = Vec::new();
        for (i, m) in me.chunks(480).enumerate() {
            let at = (i * 480) as u64;
            for (_, b) in aec.push(at, m, at + 480) {
                out.extend(b);
            }
        }
        for (_, b) in aec.flush() {
            out.extend(b);
        }
        assert_eq!(out.len(), me.len());
        let loss = 10.0 * (power(&me) / power(&out)).log10();
        assert!(loss.abs() < 1.0, "near-end speech changed by {loss:.1} dB");
    }

    #[test]
    fn waits_for_the_reference_while_system_audio_plays() {
        let reference = Arc::new(EchoReference::default());
        let mut aec = EchoCanceller::new(reference.clone()).unwrap();
        reference.push(0, &[0.1; 800]);
        // Me is ahead of Others: only the frames the reference covers come out.
        let out = aec.push(0, &[0.0; 1600], 1600);
        assert_eq!(out.iter().map(|(_, b)| b.len()).sum::<usize>(), 800);
        // Time passes without more reference: Me stops waiting after MAX_WAIT.
        let out = aec.poll(1600 + MAX_WAIT);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].0, out[0].1.len()), (800, 800));
    }

    #[test]
    fn reference_follows_the_session_clock() {
        let r = EchoReference::default();
        r.push(100, &[1.0; 10]);
        // Jitter: appended where the last chunk ended.
        r.push(105, &[2.0; 10]);
        // A gap: silence in between.
        r.push(2000, &[3.0; 10]);
        let mut out = vec![9.0; 30];
        r.read(95, &mut out);
        assert_eq!(&out[..5], &[0.0; 5]);
        assert_eq!(&out[5..15], &[1.0; 10]);
        assert_eq!(&out[15..25], &[2.0; 10]);
        assert_eq!(&out[25..], &[0.0; 5]);
        let mut out = vec![9.0; 10];
        r.read(2000, &mut out);
        assert_eq!(out, vec![3.0; 10]);
    }
}
