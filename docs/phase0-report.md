# Phase 0 report: model evaluation

- **Status:** Interim (2026-10-02). Harness done; decisions wait on the owner test set and M1 Max runs.
- **Harness:** [`eval/`](../eval/README.md) · **ADRs:** [`docs/decisions/`](decisions/README.md)

## 1. Where things stand

| SPEC 12 item | State |
|---|---|
| Evaluation harness (sherpa-onnx, reproducible in Rust) | Done |
| `eval/manifest.toml` with local clip paths | Done, 20 owner slots waiting for clips |
| FLEURS sv/en samples | Fetcher written (`fetch-fleurs`), not run: Hugging Face blocked in the build container |
| Pianissimo loads in sherpa-onnx | **Not verified**: Hugging Face blocked in the build container |
| Parakeet v3 loads in sherpa-onnx | Verified |
| Hotwords / contextual biasing | Verified working on Parakeet v3 (NeMo TDT) |
| Replacement rules | Implemented, measured on synthetic clips only |
| WER, term accuracy, wrong-engine rate per strategy | Implemented, measured on synthetic clips only |
| RTF / peak RSS on Linux CPU | Measured (cloud container, not the Ryzen box) |
| RTF / peak RSS on M1 Max, CoreML EP | Needs the owner's Mac |
| ADRs | 0001 backend proposed; routing, LID and segmentation wait for real data |

## 2. Test environment

Cloud Linux container: Intel Xeon @ 2.1 GHz, 4 vCPU, 15 GB RAM, Python 3.11, sherpa-onnx 1.13.8,
CPU provider. This is far slower than the M1 Max or the 7800X3D, so treat timings as an upper bound.

## 3. Findings

### 3.1 Backend (ADR 0001)

- `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` loads with `model_type = nemo_transducer` and
  returns per-token `ys_log_probs`, so the dual-decode confidence score (SPEC 6.3, strategy 2) is
  available without writing our own decoder.
- sherpa-onnx's NeMo loader reads model metadata from the encoder (`model_type`, `feat_dim = 128`,
  `vocab_size`, `pred_hidden`, `pred_rnn_layers`, `normalize_type`, `subsampling_factor`) and needs
  separate encoder, decoder and joiner files. Pianissimo's ONNX export must match that layout or be
  re-exported. The harness detects the layout and falls back to `onnx-asr` for the single
  `decoder_joint` layout, so either way Pianissimo can be measured.

### 3.2 Hotwords

sherpa-onnx hotwords work with the NeMo TDT model when using `modified_beam_search`. NeMo exports
lack the `bpe.vocab` the hotword tokenizer needs; deriving one from `tokens.txt` works.

| hotwords score | output for "Vi pratade med klang björn om det." |
|---|---|
| none | Vi pratade med Clang Bian om Dek. |
| 5 | Vi pratade med Klangbjörn om Deg. |
| 20 | Klangbjörn Klangbjörn Klangbjörn Klangbjörn |

The score needs tuning on real clips: too high and the biased term takes over. Beam search with
hotwords costs about 35 % more decode time than greedy (RTF 0.142 vs 0.106).

### 3.3 Speed and memory (Linux container, Parakeet v3 int8, 8 s segments)

| threads | RTF | RSS after load |
|---|---|---|
| 1 | 0.175 | 0.92 GB |
| 2 | 0.120 | 0.91 GB |
| 4 | 0.085–0.107 | 0.92 GB |

- Whisper tiny LID: RTF 0.036, about 0.2 GB. Whisper base: RTF 0.037, about 0.3 GB.
- Parakeet + Whisper tiny loaded together: 1.26 GB peak.
- **Inferred, not measured:** Pianissimo has the same architecture, so both ASR models + VAD + LID
  should land around 2.2 GB, under the 3 GB budget (SPEC 5.2).
- **Inferred, consequence for SPEC 6.2:** re-decoding the growing buffer every 800 ms costs
  RTF × buffer length. At RTF 0.1, a 15 s buffer takes about 1.5 s to re-decode, which already
  breaks the 1.5 s partial-latency target on this box. Whether the M1 Max is fast enough at the
  15 s cap is the key question for the segmentation ADR; if not, options are a shorter force-cut
  (e.g. 8–10 s) or re-decoding only the last few seconds for partials.

### 3.4 Synthetic smoke test

20 clips generated with Piper TTS (`python -m tyst_eval synth`): 5 Swedish, 5 English, 5 mixed
(sv → en → sv), 5 Swedish with English terms. 347 s audio, 170 VAD segments. With Pianissimo
unavailable, **both roles used Parakeet v3**, so this checks the plumbing and LID, not the
Swedish engine or dual-decode.

| category | Parakeet WER | LID wrong-engine | Parakeet-first wrong-engine |
|---|---|---|---|
| synth-sv | 5.2 % | 0 % | 0 % |
| synth-en | 1.4 % | 0 % | 0 % |
| synth-mixed | 3.8 % | 0 % | 0 % |
| synth-terms | 30.4 % | 0 % | 0 % |

- Term recall on synth-terms: 23 % greedy, 31 % with hotwords. Exact spelling 19 % → 27 %, and
  replacement rules bring exact spelling up to recall (23 % / 31 %).
- Whisper LID per segment: tiny 100 % sv / 100 % en, base 98 % sv / 100 % en.
- The Swedish TTS voice reads English terms with Swedish phonetics ("deploy" → "deplöi"), so the
  synth-terms numbers say little about real speech. All synthetic numbers are far too clean to
  base a decision on.

## 4. Open decisions and proposed defaults

| decision | proposed default | what settles it |
|---|---|---|
| Inference backend | sherpa-onnx (ADR 0001) | Pianissimo load test |
| Routing strategy | LID-routed with hysteresis: cheapest (≈ 1.4× one decode vs 2× for dual-decode) and 0 % wrong-engine on synthetic data | owner `mixed` + `sv-terms` clips with Pianissimo |
| LID model | Whisper tiny | owner clips; base if tiny misroutes Swedish-accented English |
| Hotwords | on, score tuned between 1.5 and 5 | owner `sv-terms` clips |
| Segmentation | SPEC defaults (500 ms silence, 15 s cut) | M1 Max RTF; see 3.3 |

## 5. Next steps (owner)

1. Allow `huggingface.co` in the project environment, or run `fetch-models` and `fetch-fleurs` on the Mac.
2. Record and correct the 20 clips (names in `eval/manifest.toml`), put them in
   `~/tyst-eval/clips/owner/`, add mixed-clip `lang_spans` and the clips' terms to `eval/terms.toml`.
3. On the M1 Max: `python3 -m tyst_eval run`, `python3 -m tyst_eval bench`, and both again with
   `--provider coreml`. Share `summary.md` and the bench JSON (metrics only, no transcript text).
