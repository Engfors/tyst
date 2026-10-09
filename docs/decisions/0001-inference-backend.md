# 0001 · Inference backend: ONNX Runtime (`ort`) with a TDT greedy decoder

- **Status:** Accepted (owner, 2026-10-02)
- **Date:** 2026-10-02

## Context

SPEC 4.1 names sherpa-onnx as the inference backend, and ONNX Runtime through the `ort` crate with
a hand-written TDT greedy decoder as the fallback if Pianissimo's ONNX export does not load in
sherpa-onnx's NeMo transducer loader. Phase 1 must reproduce the Phase 0 WER within 1 pp.

## Decision

Use **ONNX Runtime via `ort`** for ASR, with a TDT greedy decoder ported from onnx-asr's NeMo
implementation (the code the Phase 0 numbers were measured with). Run Silero VAD through `ort`
too, so there is one native dependency. sherpa-onnx is not used.

## Consequences

- Pianissimo's published export (`encoder-model`, fused `decoder_joint-model`, `nemo128.onnx`
  feature extractor, `vocab.txt`) loads as-is. No re-export step and no extra model hosting.
- We own a small decoder (greedy TDT loop, token/duration argmax, SentencePiece detokenizing).
  Phase 1 tests it against the harness outputs on the same clips.
- No hotwords/contextual biasing in v1. The vocabulary replacement rules (SPEC 9.3) still apply;
  on the owner set they lift exact term spelling to the 76–78 % term recall.
  *Amended 2026-10-09:* the vocabulary now also biases greedy decoding with Klang's phrase boosting
  (a port of NeMo's GPU-PB boosting tree, `asr::boost`) and a pure-Rust SentencePiece encoder for
  Pianissimo's `tokenizer.model` (`asr::spm`). It stays greedy and needs no other export or
  runtime. Parakeet (forced English) is boosted too: it ships the same `vocab.txt`, so the same
  tokenizer applies. Owner bench at strength 1.0: `sv-terms` term recall 75.9 → 83.3 % (WER 6.0 →
  5.7 %, `sv` unchanged at 4.8 %); `en` term recall 83.3 → 94.4 % (WER 4.5 → 4.4 %).
- Parakeet v3 (only used for forced English, ADR 0002) needs the onnx-asr export of Parakeet,
  not sherpa-onnx's, so both models share one loader.

## Alternatives

- **Re-export Pianissimo into sherpa-onnx's layout** from the `.nemo` checkpoint: brings hotwords
  and sherpa's VAD/LID, but needs NeMo tooling, a self-hosted model artifact, and a new
  measurement, since it is no longer the export that was evaluated.
- **sherpa-onnx for Parakeet + `ort` for Pianissimo:** two native stacks for no measured gain.

## Evidence

[Phase 0 report](../phase0-report.md) 3.4 and 3.5: Pianissimo through onnx-asr decodes at RTF 0.032
(4 threads) on the owner's Linux machine; sherpa-onnx cannot load its layout; Parakeet in
sherpa-onnx supports hotwords, but hotwords could not be measured for Pianissimo.
