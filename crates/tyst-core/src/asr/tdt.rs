//! Greedy TDT (token-and-duration transducer) decoding, ported from onnx-asr's NeMo transducer
//! loop, which produced the Phase 0 numbers (ADR 0001).

use super::boost::PhraseBoost;
use crate::Result;

/// The prediction network and joiner, run one encoder frame at a time.
pub trait JointNetwork {
    type State: Clone;

    fn initial_state(&self) -> Self::State;

    /// Runs decoder + joiner for one encoder frame, given the last emitted token (blank at the
    /// start). Returns the raw outputs, `vocab_size` token logits followed by duration logits,
    /// and the decoder state after consuming `prev_token`.
    fn step(&mut self, frame: &[f32], prev_token: u32, state: &Self::State) -> Result<(Vec<f32>, Self::State)>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TdtConfig {
    /// Number of token logits, blank included.
    pub vocab_size: usize,
    pub blank: u32,
    /// Tokens allowed on one frame before the decoder is forced to move on.
    pub max_tokens_per_step: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    pub tokens: Vec<u32>,
    /// Encoder frame of each token.
    pub frames: Vec<usize>,
    /// Log-probability of each token under the token softmax.
    pub logprobs: Vec<f32>,
}

impl Decoded {
    /// Mean token log-probability; SPEC 6.3's confidence. 0 is best.
    pub fn confidence(&self) -> f32 {
        if self.logprobs.is_empty() {
            return f32::NEG_INFINITY;
        }
        self.logprobs.iter().sum::<f32>() / self.logprobs.len() as f32
    }
}

/// Decodes `frames` (one feature vector per encoder frame). Duration index `k` means "advance `k`
/// frames"; the durations of NeMo's TDT models are `[0, 1, 2, 3, 4]`, so index and step coincide.
///
/// With `boost`, a frame whose unboosted argmax is a token (not blank) re-picks among the non-blank
/// tokens with the phrase bonus added (Klang's `_boosted_decoding`). Boosting therefore never adds
/// or removes an emission, the duration stays the unboosted one, and the logprob is the chosen
/// token's under the unboosted softmax.
pub fn greedy_decode<J: JointNetwork>(
    joint: &mut J,
    frames: &[Vec<f32>],
    cfg: &TdtConfig,
    boost: Option<&PhraseBoost>,
) -> Result<Decoded> {
    let mut out = Decoded::default();
    let mut state = joint.initial_state();
    let mut boost_state = 0u32;
    let mut t = 0usize;
    let mut emitted = 0usize;
    while t < frames.len() {
        let prev = out.tokens.last().copied().unwrap_or(cfg.blank);
        let (logits, next_state) = joint.step(&frames[t], prev, &state)?;
        let (token_logits, duration_logits) = logits.split_at(cfg.vocab_size);
        let mut token = argmax(token_logits) as u32;
        let step = if duration_logits.is_empty() { 0 } else { argmax(duration_logits) };
        if token != cfg.blank {
            if let Some(boost) = boost {
                token = boost.choose(token_logits, cfg.blank, &mut boost_state);
            }
            state = next_state;
            out.tokens.push(token);
            out.frames.push(t);
            out.logprobs.push(log_softmax_at(token_logits, token as usize));
            emitted += 1;
        }
        if step > 0 {
            t += step;
            emitted = 0;
        } else if token == cfg.blank || emitted == cfg.max_tokens_per_step {
            t += 1;
            emitted = 0;
        }
    }
    Ok(out)
}

/// Index of the largest value; the first one on ties, like NumPy.
pub fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

fn log_softmax_at(v: &[f32], i: usize) -> f32 {
    let max = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = v.iter().map(|x| (x - max).exp()).sum();
    v[i] - max - sum.ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scripted joint: each call pops the next (token, duration) pair and checks its inputs.
    struct Script {
        steps: Vec<(u32, usize)>,
        calls: Vec<(usize, u32, u32)>, // (frame marker, prev token, state)
        vocab: usize,
    }

    impl JointNetwork for Script {
        type State = u32;
        fn initial_state(&self) -> u32 {
            0
        }
        fn step(&mut self, frame: &[f32], prev: u32, state: &u32) -> Result<(Vec<f32>, u32)> {
            self.calls.push((frame[0] as usize, prev, *state));
            let (tok, dur) = self.steps.remove(0);
            let mut logits = vec![0.0; self.vocab + 5];
            logits[tok as usize] = 5.0;
            logits[self.vocab + dur] = 5.0;
            Ok((logits, state + 1))
        }
    }

    fn frames(n: usize) -> Vec<Vec<f32>> {
        (0..n).map(|i| vec![i as f32]).collect()
    }

    const BLANK: u32 = 9;
    const CFG: TdtConfig = TdtConfig { vocab_size: 10, blank: BLANK, max_tokens_per_step: 2 };

    #[test]
    fn follows_durations_and_blanks() {
        let mut j = Script { steps: vec![(3, 2), (BLANK, 0), (4, 0), (5, 1)], calls: vec![], vocab: 10 };
        let d = greedy_decode(&mut j, &frames(4), &CFG, None).unwrap();
        assert_eq!(d.tokens, vec![3, 4, 5]);
        assert_eq!(d.frames, vec![0, 3, 3]);
        // frame 0 -> skip 2 -> blank on frame 2 advances 1 -> frame 3 emits twice, then moves past the end
        let visited: Vec<usize> = j.calls.iter().map(|c| c.0).collect();
        assert_eq!(visited, vec![0, 2, 3, 3]);
    }

    #[test]
    fn state_and_prev_token_only_advance_on_emission() {
        let mut j = Script { steps: vec![(BLANK, 1), (7, 1), (BLANK, 1)], calls: vec![], vocab: 10 };
        greedy_decode(&mut j, &frames(3), &CFG, None).unwrap();
        // The joint returns state + 1; it is kept only after the emission on frame 1.
        assert_eq!(j.calls, vec![(0, BLANK, 0), (1, BLANK, 0), (2, 7, 1)]);
    }

    #[test]
    fn caps_tokens_per_frame() {
        let mut j = Script { steps: vec![(1, 0), (2, 0), (3, 0), (BLANK, 0)], calls: vec![], vocab: 10 };
        let d = greedy_decode(&mut j, &frames(2), &CFG, None).unwrap();
        assert_eq!(d.tokens, vec![1, 2, 3]);
        assert_eq!(d.frames, vec![0, 0, 1]);
    }

    #[test]
    fn logprobs_are_normalized() {
        let mut j = Script { steps: vec![(2, 1)], calls: vec![], vocab: 10 };
        let d = greedy_decode(&mut j, &frames(1), &CFG, None).unwrap();
        let p = d.logprobs[0].exp();
        assert!((p - 5f32.exp() / (5f32.exp() + 9.0)).abs() < 1e-5);
        assert!(d.confidence() < 0.0);
    }

    /// Joint returning fixed token logits per call, with the duration argmax at `dur`.
    struct Fixed {
        steps: Vec<(Vec<f32>, usize)>,
    }

    impl JointNetwork for Fixed {
        type State = ();
        fn initial_state(&self) {}
        fn step(&mut self, _frame: &[f32], _prev: u32, _state: &()) -> Result<(Vec<f32>, ())> {
            let (mut logits, dur) = self.steps.remove(0);
            let mut durations = vec![0.0; 5];
            durations[dur] = 1.0;
            logits.extend(durations);
            Ok((logits, ()))
        }
    }

    #[test]
    fn boost_flips_close_tokens_but_not_blanks_or_durations() {
        use super::super::boost::{BoostTree, CONTEXT_SCORE, DEPTH_SCALING, PhraseBoost};
        // Phrase "2 3". Frame 0: token 1 leads 2 by 0.5. Frame 1: blank leads. Frame 2: 3 is close.
        let step = |lead: usize, close: usize, gap: f32, dur: usize| {
            let mut l = vec![0.0; 10];
            l[lead] = 4.0;
            l[close] = 4.0 - gap;
            (l, dur)
        };
        let script = || Fixed { steps: vec![step(1, 2, 0.5, 1), step(BLANK as usize, 2, 0.1, 1), step(4, 3, 0.5, 1)] };
        let plain = greedy_decode(&mut script(), &frames(3), &CFG, None).unwrap();
        assert_eq!(plain.tokens, vec![1, 4]);
        let boost = PhraseBoost { tree: BoostTree::new(&[vec![2, 3]], CONTEXT_SCORE, DEPTH_SCALING), alpha: 1.0 };
        let boosted = greedy_decode(&mut script(), &frames(3), &CFG, Some(&boost)).unwrap();
        assert_eq!(boosted.tokens, vec![2, 3]);
        assert_eq!(boosted.frames, plain.frames);
        // The logprob is the chosen token's under the unboosted softmax, so lower than the leader's.
        assert!(boosted.logprobs[0] < plain.logprobs[0]);
    }
}
