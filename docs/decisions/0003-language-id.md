# 0003 · Language ID: none in v1

- **Status:** Proposed
- **Date:** 2026-10-02

## Context

LID was only needed to pick an engine per segment (SPEC 5.1, 6.3).

## Decision

No spoken-language-ID model in v1. Language for labels comes from the user's forced setting or a
text-based guess on the final transcript.

## Consequences

- Saves about 0.2–0.3 GB RSS and the extra decode cost per segment.
- If routing returns (ADR 0002), Whisper tiny is the default: Whisper base was no better.

## Evidence

[Phase 0 report](../phase0-report.md) 3.2: Whisper tiny labels 80.9 % of Swedish segments as
Swedish-like and 99.2 % of English segments correctly. Base gets 81.2 % / 98.4 %.
