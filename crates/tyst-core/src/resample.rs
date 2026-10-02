//! Polyphase resampling to 16 kHz.
//!
//! A streaming port of SciPy's `resample_poly(x, up, down, window=("kaiser", 5.0))`, which the
//! Phase 0 harness used to load clips. Matching it keeps Phase 1 WER comparable to Phase 0, and
//! the same code handles live capture, where samples arrive in chunks.

/// Target sample rate of the whole pipeline.
pub const SAMPLE_RATE: u32 = 16_000;

/// Converts a stream at `from` Hz to `to` Hz. Feed it with [`push`](Self::push) and finish with
/// [`flush`](Self::flush); the concatenated output equals the offline result.
pub struct Resampler {
    up: usize,
    down: usize,
    /// FIR taps, already scaled by `up`.
    taps: Vec<f64>,
    /// Zeros SciPy puts in front of the filter so the output is centred.
    pre_pad: usize,
    /// Output samples of the full convolution SciPy drops at the start.
    pre_remove: usize,
    /// Input history; `history[0]` is input sample number `history_start`.
    history: Vec<f32>,
    history_start: usize,
    /// Total input samples received.
    received: usize,
    /// Next output index (before `pre_remove`).
    next_out: usize,
    passthrough: bool,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        assert!(from > 0 && to > 0, "sample rates must be positive");
        let g = gcd(from as usize, to as usize);
        let (up, down) = (to as usize / g, from as usize / g);
        let max_rate = up.max(down);
        let half_len = 10 * max_rate;
        let taps = firwin_kaiser(2 * half_len + 1, 1.0 / max_rate as f64, 5.0)
            .into_iter()
            .map(|t| t * up as f64)
            .collect::<Vec<_>>();
        let pre_pad = down - half_len % down;
        let pre_remove = (half_len + pre_pad) / down;
        Self {
            up,
            down,
            taps,
            pre_pad,
            pre_remove,
            history: Vec::new(),
            history_start: 0,
            received: 0,
            next_out: pre_remove,
            passthrough: up == down,
        }
    }

    /// Resamples a whole buffer at once.
    pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
        let mut r = Self::new(from, to);
        let mut out = r.push(samples);
        out.extend(r.flush());
        out
    }

    /// Adds input and returns every output sample that no longer depends on future input.
    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            self.received += input.len();
            return input.to_vec();
        }
        self.history.extend_from_slice(input);
        self.received += input.len();
        let mut out = Vec::with_capacity(input.len() * self.up / self.down + 1);
        loop {
            let t = self.next_out * self.down;
            // Newest input sample this output touches.
            if t < self.pre_pad {
                out.push(self.output_at(t));
                self.next_out += 1;
                continue;
            }
            let newest = (t - self.pre_pad) / self.up;
            if newest >= self.received {
                break;
            }
            out.push(self.output_at(t));
            self.next_out += 1;
        }
        self.trim();
        out
    }

    /// Ends the stream: emits the remaining outputs, treating input past the end as silence.
    pub fn flush(&mut self) -> Vec<f32> {
        if self.passthrough {
            return Vec::new();
        }
        let total = (self.received * self.up).div_ceil(self.down) + self.pre_remove;
        let mut out = Vec::new();
        while self.next_out < total {
            out.push(self.output_at(self.next_out * self.down));
            self.next_out += 1;
        }
        self.history.clear();
        self.history_start = self.received;
        out
    }

    /// Output of the full (un-trimmed) convolution at upsampled position `t`.
    fn output_at(&self, t: usize) -> f32 {
        // y[t] = sum_i taps[t - pre_pad - i*up] * x[i]
        let n = self.taps.len();
        let base = t as isize - self.pre_pad as isize;
        // Smallest i with base - i*up <= n-1, largest i with base - i*up >= 0.
        let lo = (base - n as isize + 1).max(0) as usize;
        let i_min = lo.div_ceil(self.up);
        if base < 0 {
            return 0.0;
        }
        let i_max = base as usize / self.up;
        let mut acc = 0.0f64;
        let mut i = i_min.max(self.history_start);
        while i <= i_max && i < self.received {
            let j = base as usize - i * self.up;
            acc += self.taps[j] * self.history[i - self.history_start] as f64;
            i += 1;
        }
        acc as f32
    }

    /// Drops input that no future output needs.
    fn trim(&mut self) {
        let t = self.next_out * self.down;
        let base = t as isize - self.pre_pad as isize;
        let lo = (base - self.taps.len() as isize + 1).max(0) as usize;
        let keep_from = lo.div_ceil(self.up).min(self.received);
        if keep_from > self.history_start {
            self.history.drain(..keep_from - self.history_start);
            self.history_start = keep_from;
        }
    }
}

/// Averages interleaved channels into mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `scipy.signal.firwin(numtaps, cutoff, window=("kaiser", beta))` for a low-pass filter with the
/// cutoff relative to Nyquist, scaled to unit gain at DC.
fn firwin_kaiser(numtaps: usize, cutoff: f64, beta: f64) -> Vec<f64> {
    let alpha = (numtaps - 1) as f64 / 2.0;
    let i0_beta = bessel_i0(beta);
    let mut h: Vec<f64> = (0..numtaps)
        .map(|n| {
            let m = n as f64 - alpha;
            let sinc = if m == 0.0 {
                1.0
            } else {
                let x = std::f64::consts::PI * cutoff * m;
                x.sin() / x
            };
            let r = m / alpha;
            let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
            cutoff * sinc * w
        })
        .collect();
    let sum: f64 = h.iter().sum();
    h.iter_mut().for_each(|x| *x /= sum);
    h
}

/// Modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..200 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn output_length_matches_scipy() {
        for (from, n) in [(48_000, 48_000), (44_100, 44_100), (8_000, 1234), (22_050, 777)] {
            let out = Resampler::resample(&vec![0.1; n], from, SAMPLE_RATE);
            let expected = (n * 16_000).div_ceil(from as usize);
            assert_eq!(out.len(), expected, "from {from}");
        }
    }

    #[test]
    fn identity_rate_passes_through() {
        let x = sine(440.0, 16_000, 0.1);
        assert_eq!(Resampler::resample(&x, 16_000, 16_000), x);
    }

    #[test]
    fn streaming_equals_offline() {
        let x = sine(300.0, 44_100, 0.5);
        let offline = Resampler::resample(&x, 44_100, SAMPLE_RATE);
        let mut r = Resampler::new(44_100, SAMPLE_RATE);
        let mut streamed = Vec::new();
        for chunk in x.chunks(1000) {
            streamed.extend(r.push(chunk));
        }
        streamed.extend(r.flush());
        assert_eq!(streamed.len(), offline.len());
        for (a, b) in streamed.iter().zip(&offline) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn keeps_passband_and_removes_aliases() {
        // 1 kHz survives 48k -> 16k at the same amplitude; 12 kHz (above the new Nyquist) is removed.
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let pass = Resampler::resample(&sine(1000.0, 48_000, 1.0), 48_000, SAMPLE_RATE);
        let stop = Resampler::resample(&sine(12_000.0, 48_000, 1.0), 48_000, SAMPLE_RATE);
        let mid = |v: &Vec<f32>| v[2000..v.len() - 2000].to_vec();
        assert!((rms(&mid(&pass)) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!(rms(&mid(&stop)) < 0.01);
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }
}
