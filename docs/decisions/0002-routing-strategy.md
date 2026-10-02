# 0002 · Language routing: fixed Pianissimo, English on request

- **Status:** Accepted (owner, 2026-10-02)
- **Date:** 2026-10-02

## Context

SPEC 6.3 proposes three multi-model strategies (LID-routed, dual-decode, Parakeet-first) because
Pianissimo's English and code-switching performance was not established.

## Decision

The default `LanguageRouter` sends every segment to **Pianissimo**. When the user forces English
for a session (meeting window or pill), segments go to **Parakeet v3**, loaded lazily on first use.
The `LanguageRouter` trait stays, so a multi-model strategy can come back without pipeline changes.

## Consequences

- One model loaded by default: 1.86 GB peak RSS instead of 2.75–2.85 GB with all models.
- Decode cost is one pass per segment (RTF 0.042 in the run) instead of 0.056–0.077 for routing.
- No hysteresis or language-flip logic is needed in v1. The Markdown `languages` field and the
  per-turn language tag (SPEC 7, 8.3) need another source; a cheap text-based guess on the final
  text is enough for a label.
- Risk: the English test set is small (5 owner clips, 25 FLEURS). If native-English meetings
  turn out worse in daily use, forcing English is the fallback, and dual-decode is the next
  strategy to revisit (best on `fleurs-en`).

## Evidence

[Phase 0 report](../phase0-report.md) 3.1: Pianissimo alone has 7.2 % WER on the 20 owner clips
(LID-routed 7.3, dual 7.4, Parakeet-first 7.5, Parakeet alone 12.3), the lowest WER on both mixed
sets, and 5.7 % vs Parakeet's 5.1 % on the owner's English clips.
