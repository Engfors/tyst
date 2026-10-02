# Phase 0 report: model evaluation

- **Status:** Draft for owner review (2026-10-02). Owner test set, FLEURS, Linux and M1 Max benches measured.
- **Harness:** [`eval/`](../eval/README.md) · **ADRs:** [`docs/decisions/`](decisions/README.md)

## 1. Summary

**Pianissimo alone is the best or tied-best engine on every real-speech category, English included.**
Every routing strategy costs 35–95 % more decode time than Pianissimo alone and none lowers WER on
the owner clips. The proposal is to ship v1 with a fixed Pianissimo router, keep Parakeet v3 as an
optional engine for a forced English session, and drop language ID for now (ADR 0002, 0003).

Pianissimo's published ONNX export uses the onnx-asr layout, which sherpa-onnx cannot load. The
measured numbers come from that export through ONNX Runtime, so the proposal for the backend is
the SPEC's `ort` fallback with a hand-written TDT greedy decoder (ADR 0001).

## 2. Data and setup

| set | clips | content |
|---|---|---|
| owner `sv`, `en`, `mixed`, `sv-terms` | 5 each | owner recordings with hand-corrected references (interview-style manuscripts) |
| `fleurs-sv`, `fleurs-en` | 25 each | FLEURS test utterances (read speech) |
| `fleurs-mixed` | 5 | FLEURS sv → en → sv concatenations with known language spans |
| `synth-*` | 20 | Piper TTS smoke tests, ignored for decisions |

Runs: owner's Linux machine (CachyOS, x86_64), CPU provider, 4 threads, sherpa-onnx 1.13.8.
Engines: `sv` = `pianissimo-sv-int8` (loaded through onnx-asr), `en` = `parakeet-v3-int8` (sherpa-onnx).
Three runs: owner clips with Whisper tiny LID, all 95 clips with Whisper tiny, all 95 with Whisper base.
WER is on normalized text with digits spelled out.

## 3. Results

### 3.1 WER by strategy (greedy, raw output, Whisper tiny LID)

| category | Pianissimo only | Parakeet only | LID-routed | dual-decode | Parakeet-first |
|---|---|---|---|---|---|
| owner, all 20 | **7.2** | 12.3 | 7.3 | 7.4 | 7.5 |
| owner `sv` | 6.2 | 15.0 | **6.1** | 6.9 | 6.2 |
| owner `en` | 5.7 | **5.1** | **5.1** | 6.0 | **5.1** |
| owner `mixed` | **9.0** | 13.9 | 10.8 | 9.1 | 10.5 |
| owner `sv-terms` | 7.8 | 15.7 | **7.6** | 7.8 | 8.5 |
| `fleurs-sv` | **10.4** | 19.6 | **10.4** | 11.6 | **10.4** |
| `fleurs-en` | 23.3 | 27.7 | 27.7 | **22.2** | 24.8 |
| `fleurs-mixed` | **26.0** | 38.5 | 32.2 | 27.6 | 32.2 |
| RTF (owner, all) | **0.042** | 0.036 | 0.056 | 0.077 | 0.062 |

- Pianissimo's English is close to Parakeet's on the owner's English clips (5.7 vs 5.1) and better
  on native-speaker FLEURS English (23.3 vs 27.7). Pattern A (language switch between turns) is
  handled without routing: Pianissimo alone has the lowest WER on both mixed sets.
- Pattern B (English terms inside Swedish) also works without routing: on `sv-terms` Pianissimo has
  7.8 % WER and 77.8 % term recall, against 15.7 % and 66.7 % for Parakeet.
- Routing only wins inside noise (≤ 0.6 pp, 5 clips) and always costs extra decode time.
- **Unexplained:** FLEURS English WER is high for both engines (22–28 %). It is likely a scoring
  artefact in how FLEURS transcripts write things rather than an engine problem, but this has not
  been checked yet. It doesn't change the ranking.

### 3.2 Language ID and routing errors

| | Whisper tiny | Whisper base |
|---|---|---|
| sv segments correct (all 95 clips) | 80.9 % | 81.2 % |
| en segments correct (all 95 clips) | 99.2 % | 98.4 % |
| `fleurs-mixed` wrong-engine rate, LID-routed | 6.7 % | 13.3 % |

- Whisper LID labels about one Swedish segment in five as something other than Swedish
  (Scandinavian labels already count as Swedish). Base is not better than tiny.
- **Measurement gap:** the owner `mixed` clips were scored without `lang_spans`, since Pianissimo-only
  shows 0 % wrong-engine there. The wrong-engine rates for owner `mixed` (14–45 %) are therefore
  not meaningful; WER for those clips is unaffected.

### 3.3 Vocabulary post-processing

Replacement rules and canonical spelling raise exact-spelling term accuracy to the term recall
(owner set: 70.8 % → 76.4 % for Pianissimo) and change WER by ≤ 0.1 pp. Hotwords could not be
measured for Pianissimo: sherpa-onnx contextual biasing needs the sherpa layout (see 3.5).

### 3.4 Speed and memory

From the run logs (4 threads, VAD segments): Pianissimo RTF 0.042–0.046, Parakeet 0.036, model
load ≈ 1.3 s and 1.1 s. At RTF ≈ 0.045, re-decoding a full 15 s segment for a partial takes about
0.7 s, inside the 1.5 s partial target, so the SPEC segmentation defaults hold on this machine.

Bench (`python -m tyst_eval bench`, 120 s of speech in 8 s pieces, each configuration in a fresh process):

| models loaded | threads | RTF | RSS after load | peak RSS |
|---|---|---|---|---|
| Pianissimo | 1 | 0.094 | 1.62 GB | 1.86 GB |
| Pianissimo | 4 | 0.032 | 1.62 GB | 1.86 GB |
| Parakeet | 1 | 0.097 | 1.05 GB | 1.20 GB |
| Parakeet | 4 | 0.034 | 1.09 GB | 1.24 GB |
| Pianissimo + Parakeet + Whisper tiny | 1 | 0.095 / 0.097 / 0.025 | 2.38 GB | 2.85 GB |
| Pianissimo + Parakeet + Whisper tiny | 4 | 0.033 / 0.034 / 0.017 | 2.29 GB | 2.75 GB |

- With everything loaded, peak RSS is 2.75–2.85 GB before VAD, audio buffers and the app itself.
  That is at the edge of the 3 GB budget (SPEC 5.2). Pianissimo alone peaks at 1.86 GB.
- Pianissimo loaded through onnx-asr uses about 0.55 GB more than Parakeet through sherpa-onnx,
  although both are int8 0.6B models. The fp32 feature extractor and ORT arena settings are the
  likely cause (inferred, not measured); Phase 1 should check this in the `ort` implementation.
- Single-threaded RTF is about 0.095, so even on one core a 15 s re-decode takes about 1.4 s.

M1 Max (macOS 26.6, CPU provider; CoreML not run):

| models loaded | threads | RTF | RSS after load | peak RSS |
|---|---|---|---|---|
| Pianissimo | 1 | 0.129 | 1.15 GB | 1.34 GB |
| Pianissimo | 4 | 0.043 | 1.21 GB | 1.40 GB |
| Parakeet | 1 | 0.071 | 1.20 GB | 1.31 GB |
| Parakeet | 4 | 0.031 | 1.24 GB | 1.35 GB |
| Pianissimo + Parakeet + Whisper tiny | 1 | 0.132 / 0.071 / 0.024 | 2.04 GB | 2.43 GB |
| Pianissimo + Parakeet + Whisper tiny | 4 | 0.043 / 0.030 / 0.010 | 2.19 GB | 2.53 GB |

- On the Mac, Pianissimo through onnx-asr is slower than Parakeet through sherpa-onnx (0.129 vs
  0.071 single-threaded), while on Linux they were equal. sherpa-onnx's feature extraction and
  decoder loop are native, and onnx-asr's run in Python/NumPy, so part of the gap is likely the
  harness rather than the model (inferred). Phase 1's Rust decoder will show the real number.
- **Latency target holds:** a full 15 s re-decode takes 0.64 s with 4 threads, under the 1.5 s
  partial target and the 1.0 s final target.
- **CPU target is at risk (estimate):** re-decoding the whole growing buffer costs CPU time that
  grows with segment length. Simulated cores used per second of speech (partials + final decode,
  single-thread RTF 0.129, the M1 Max number):

  | partial interval | 2 s turn | 4 s turn | 8 s turn | 15 s turn |
  |---|---|---|---|---|
  | fixed 0.8 s (SPEC) | 0.28 | 0.39 | 0.84 | 1.31 |
  | `max(0.8 s, L/2)` | 0.28 | 0.40 | 0.35 | 0.42 |
  | `max(0.8 s, L)` | 0.28 | 0.31 | 0.32 | 0.34 |

  The SPEC 6.2 target is 30 % of one core on average over a meeting, silence included. Even short
  turns land near 0.3 cores while someone talks, so the target holds only if speech fills well
  under the whole meeting, or if the Rust decoder is faster than the onnx-asr harness (Parakeet in
  sherpa-onnx runs at 0.071 on the same Mac). ADR 0004 proposes `max(0.8 s, L/2)`.

### 3.5 Backend compatibility

- Pianissimo int8 (`KlangAI/pianissimo-sv-onnx`, revision `63730c6`) ships `encoder-model.int8.onnx`,
  `decoder_joint-model.int8.onnx`, `nemo128.onnx` (feature extractor) and `vocab.txt`. That is
  the onnx-asr layout: decoder and joiner are fused, and the sherpa-onnx metadata is missing, so
  sherpa-onnx's NeMo transducer loader cannot use it. The harness loaded it through onnx-asr.
- Parakeet v3 (sherpa-onnx's export) loads in sherpa-onnx and supports hotwords (verified in the
  build container: "klang björn" → "Klangbjörn" at score 5).

## 4. Proposed decisions

| decision | proposal | ADR |
|---|---|---|
| Inference backend | ONNX Runtime via `ort`, hand-written TDT greedy decoder ported from onnx-asr; Silero VAD via `ort` | [0001](decisions/0001-inference-backend.md) |
| Routing | fixed Pianissimo; the user can force English per session, which uses Parakeet v3 | [0002](decisions/0002-routing-strategy.md) |
| Language ID | none in v1 | [0003](decisions/0003-language-id.md) |
| Segmentation | SPEC defaults (500 ms end silence, 15 s force-cut, 300 ms minimum), partial interval `max(0.8 s, L/2)` for buffer length L | [0004](decisions/0004-segmentation.md) |

## 5. Still open

1. Owner confirmation of ADRs 0001–0004.
2. Optional: CoreML execution provider on the M1 Max; not needed for the targets above.
3. Optional: rerun the owner set with `lang_spans` on the `mixed` clips to get a real wrong-engine rate.
   This only matters if routing is kept.

## Appendix: build-container smoke test

Before the owner runs, the harness was exercised in a cloud container (Xeon 2.1 GHz, 4 vCPU) on
synthetic Piper TTS clips, with Parakeet v3 in both roles. Parakeet RTF 0.085–0.175 (4 → 1
threads), about 0.92 GB RSS after load. Whisper tiny LID: RTF 0.036, about 0.2 GB.
