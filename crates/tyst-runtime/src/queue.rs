//! The bounded audio queue between a capture thread and its worker, shared by meetings and
//! dictation. Sources push into an unbounded channel of their own; the capture thread moves chunks
//! on to the worker only while the worker keeps up, so a recognizer slower than real time cannot
//! grow memory without limit (SPEC 11).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

use tyst_platform::AudioChunk;

/// Audio waiting for the worker beyond this: quiet chunks are dropped (SPEC 11: 3-hour
/// meetings without memory growth).
pub(crate) const SOFT_BACKLOG_MS: u64 = 10_000;
/// Beyond this every chunk is dropped, with a warning, until the worker catches up. About 23 MB
/// of 48 kHz mono audio per channel.
pub(crate) const HARD_BACKLOG_MS: u64 = 120_000;
/// RMS below this counts as silence for dropping (about -54 dBFS).
const SILENCE_RMS: f32 = 0.002;

/// What the capture thread does with a chunk, given how much audio already waits.
#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Keep,
    DropSilence,
    DropBehind,
}

fn admit(backlog_ms: u64, rms: f32) -> Admit {
    if backlog_ms >= HARD_BACKLOG_MS {
        Admit::DropBehind
    } else if backlog_ms >= SOFT_BACKLOG_MS && rms < SILENCE_RMS {
        Admit::DropSilence
    } else {
        Admit::Keep
    }
}

pub(crate) fn chunk_ms(c: &AudioChunk) -> u64 {
    c.samples.len() as u64 * 1000 / c.sample_rate.max(1) as u64
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The capture side of the queue. The worker subtracts [`chunk_ms`] from `backlog` for every
/// chunk it takes.
pub(crate) struct Queue {
    /// For logs: the channel or session the queue feeds.
    label: String,
    tx: Sender<AudioChunk>,
    backlog: Arc<AtomicU64>,
    /// Called once each time the queue starts dropping everything, to tell the user.
    on_behind: Box<dyn FnMut() + Send>,
    /// Dropping everything (hard limit reached) until the worker catches up.
    behind: bool,
    dropped_ms: u64,
}

impl Queue {
    pub(crate) fn new(
        label: String,
        tx: Sender<AudioChunk>,
        backlog: Arc<AtomicU64>,
        on_behind: Box<dyn FnMut() + Send>,
    ) -> Self {
        Self { label, tx, backlog, on_behind, behind: false, dropped_ms: 0 }
    }

    pub(crate) fn push(&mut self, chunk: AudioChunk) {
        let ms = chunk_ms(&chunk);
        let backlog = self.backlog.load(Ordering::SeqCst);
        if self.behind && backlog < SOFT_BACKLOG_MS {
            self.behind = false;
            log::warn!("{}: recognizer caught up; {} ms of audio were skipped", self.label, self.dropped_ms);
            self.dropped_ms = 0;
        }
        match admit(backlog, rms(&chunk.samples)) {
            Admit::Keep if !self.behind => {
                self.backlog.fetch_add(ms, Ordering::SeqCst);
                let _ = self.tx.send(chunk);
            }
            Admit::DropSilence => {}
            Admit::Keep | Admit::DropBehind => {
                if !self.behind {
                    self.behind = true;
                    log::warn!("{}: recognizer is {} ms behind, skipping audio", self.label, backlog);
                    (self.on_behind)();
                }
                self.dropped_ms += ms;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::Instant;

    use tyst_platform::Channel;

    use super::*;

    #[test]
    fn queue_drops_silence_then_everything_when_far_behind() {
        assert_eq!(admit(0, 0.0), Admit::Keep);
        assert_eq!(admit(SOFT_BACKLOG_MS, 0.5), Admit::Keep);
        assert_eq!(admit(SOFT_BACKLOG_MS, 0.0), Admit::DropSilence);
        assert_eq!(admit(HARD_BACKLOG_MS, 0.5), Admit::DropBehind);
    }

    #[test]
    fn queue_stays_bounded_and_warns_once() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let w = warnings.clone();
        let (tx, rx) = mpsc::channel();
        let backlog = Arc::new(AtomicU64::new(0));
        let mut q = Queue::new(
            "test".into(),
            tx,
            backlog.clone(),
            Box::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let chunk = || AudioChunk {
            channel: Channel::Others,
            sample_rate: 48_000,
            samples: vec![0.1; 4800],
            captured_at: Instant::now(),
        };
        // A worker that never takes anything: 10 minutes of speech at 48 kHz, 100 ms chunks.
        for _ in 0..6000 {
            q.push(chunk());
        }
        assert_eq!(backlog.load(Ordering::SeqCst), HARD_BACKLOG_MS);
        assert_eq!(rx.try_iter().count() as u64, HARD_BACKLOG_MS / 100);
        assert_eq!(warnings.load(Ordering::SeqCst), 1);
        // The worker catches up: audio flows again.
        backlog.store(0, Ordering::SeqCst);
        q.push(chunk());
        assert!(!q.behind);
        assert_eq!(rx.try_iter().count(), 1);
    }
}
