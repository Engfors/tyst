# 0004 · Segmentation parameters

- **Status:** Proposed (pending M1 Max confirmation)
- **Date:** 2026-10-02

## Decision

Keep the SPEC 6.2 defaults: Silero VAD threshold 0.5, end of speech after 500 ms of silence,
segments under 300 ms dropped, force-cut at 15 s, partial re-decode every 800 ms.

## Consequences

- Worst-case partial cost is one 15 s re-decode: about 0.5 s with 4 threads and 1.4 s on one core
  on the owner's Linux machine (RTF 0.032 / 0.094).
- If the M1 Max CPU path is much slower, lower the force-cut to 10 s before reducing the partial rate.

## Evidence

[Phase 0 report](../phase0-report.md) 3.4. All Phase 0 WER numbers were measured with these
parameters.
