# 0001 · Inference backend: sherpa-onnx

- **Status:** Proposed (pending Pianissimo load test)
- **Date:** 2026-10-02

## Context

SPEC 4.1 names sherpa-onnx (C API or `sherpa-rs`) as the inference backend, with ONNX Runtime via
the `ort` crate plus a hand-written TDT greedy decoder as the fallback if Pianissimo's ONNX export
does not load in sherpa-onnx's NeMo transducer loader. The router also needs per-token
log-probabilities (dual-decode, SPEC 6.3) and contextual biasing (SPEC 9.3).

## Decision

Use **sherpa-onnx** (`model_type = nemo_transducer`) for ASR, Silero VAD and Whisper language ID,
provided Pianissimo loads in it. If Pianissimo's published export does not, the preferred fix is to
re-export it into sherpa-onnx's layout from the `.nemo` checkpoint rather than switch backends; the
`ort` path stays the last resort.

## Consequences

- One native dependency covers ASR, VAD, LID and hotwords; no custom decoder to maintain.
- Hotwords need a `bpe.vocab`, which NeMo exports lack. The harness derives one from `tokens.txt`
  (`eval/tyst_eval/engines.py`, `write_bpe_vocab`); Phase 1 must do the same in Rust.
- Hotword decoding uses `modified_beam_search`, about 35 % slower than greedy on the test box.

## Evidence (Linux container, sherpa-onnx 1.13.8, see docs/phase0-report.md)

- Parakeet TDT 0.6B v3 int8 (sherpa-onnx export) loads and decodes; results include
  `ys_log_probs`, so dual-decode confidence needs no custom decoder.
- Hotwords work with the NeMo TDT model: "klang björn" → "Klangbjörn" at score 5.
- **Not yet verified:** Pianissimo, because Hugging Face is unreachable from the build container.
  The harness auto-detects either layout and reports a load failure in the run summary.
