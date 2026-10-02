# 0004 · Segmentation parameters

- **Status:** Accepted (owner, 2026-10-02)
- **Date:** 2026-10-02

## Decision

- Silero VAD threshold 0.5, end of speech after 500 ms of silence, segments under 300 ms dropped,
  force-cut at 15 s (SPEC 6.2 defaults; all Phase 0 WER numbers were measured with them).
- Partial re-decode interval grows with the open segment: `max(0.8 s, L / 2)` for buffer length
  `L`. That is every 0.8 s for the first 1.6 s, then at about 2.4, 3.6, 5.4, 8.1 and 12.2 s.

## Consequences

- Latency: the slowest partial (a full 15 s buffer) takes about 0.64 s on the M1 Max with 4
  threads (RTF 0.043), so partials stay within 1.5 s and finals within 1.0 s.
- CPU: with a fixed 0.8 s interval, the cost grows with segment length and passes one core for
  15 s segments. With the growing interval it stays at 0.28–0.42 cores per second of speech,
  final decode included (report 3.4, simulated with single-thread RTF 0.129). The 30 %-of-a-core
  meeting average then depends on how much of the meeting is silence and on how fast the Rust
  decoder is. This is an estimate; Phase 1's `tyst-cli live` measures it, and the target or this
  policy gets revisited then.
- Partial text updates less often late in a long segment (gaps of 2–4 s after 5 s of speech).

## Evidence

[Phase 0 report](../phase0-report.md) 3.4: Pianissimo RTF on the M1 Max is 0.129 single-threaded
and 0.043 with 4 threads; on the owner's Linux machine 0.094 and 0.032.
