# Phase 1 report: core pipeline and CLI

- **Status:** All Phase 1 targets met on the owner's machine (section 4); awaiting the owner's acceptance.
- **Code:** `crates/tyst-core`, `crates/tyst-platform`, `crates/tyst-cli` · **CI:** `.github/workflows/ci.yml`

## 1. What exists

| SPEC 12 item | Where |
|---|---|
| Workspace, `tyst-core`, `tyst-cli`, CI (fmt, clippy, tests) on macOS + Linux | `Cargo.toml`, `.github/workflows/ci.yml` |
| File input and live mic input | `tyst_core::audio_file`, `tyst_platform::mic` (cpal) |
| VAD → segments with partial/final events | `tyst_core::vad` (Silero via `ort`), `segmenter`, `pipeline` |
| Router | `tyst_core::router` (ADR 0002: fixed Pianissimo, Parakeet v3 lazily for forced English) |
| ASR | `tyst_core::asr` (ONNX Runtime + TDT greedy decoder, ADR 0001) |
| Vocabulary | `tyst_core::vocabulary` (replacements + term spelling, SPEC 9.3) |
| Markdown writer + journal | `tyst_core::markdown`, `tyst_core::journal` (SPEC 6.6, 7) |
| `transcribe <file>`, `live --mic`, `bench <manifest>` | `tyst-cli`, plus `models` (fetch/verify/list) and `recover` |
| Unit tests: segmenter, router hysteresis, vocabulary, Markdown, journal recovery | 55 unit tests in `tyst-core` |

Models are pinned in `models/models.toml` (Hugging Face repo, revision, size and SHA-256 per file) and
downloaded only by `tyst-cli models fetch`. The core crate has no network code.

## 2. WER parity with the Phase 0 harness

The Phase 0 harness (`python -m tyst_eval run --engines sv=pianissimo-sv-int8,en=pianissimo-sv-int8 --lid "" --decode greedy`)
and `tyst-cli bench` ran on the same machine, the same 55 FLEURS clips (selected by the harness's own
`fetch-fleurs` logic) and the same Silero VAD weights.

| category | clips | harness WER | `tyst-cli` WER | Δ pp |
|---|---|---|---|---|
| all FLEURS | 55 | 20.6 | 20.6 | +0.0 |
| `fleurs-sv` | 25 | 10.2 | 10.4 | +0.2 |
| `fleurs-en` | 25 | 25.1 | 25.1 | +0.0 |
| `fleurs-mixed` | 5 | 25.8 | 25.6 | −0.2 |

47 of the 55 transcripts are identical word for word; the other 8 differ in one token each (e.g.
"Shup."/"Chub.", a trailing comma), which is int8 arithmetic in two ONNX Runtime builds. The decoder
port is faithful. The owner clips (the SPEC's acceptance set) are not in the container; section 4.

### VAD model

The first run used Silero VAD v5 and missed quiet English speech after loud Swedish speech
(`fleurs-mixed` 40.5 % WER). Silero v4, the model sherpa-onnx ships and Phase 0 used, finds it. The
manifest pins v4 (`csukuangfj/vad`, which gives speech probabilities identical to sherpa-onnx's
`silero_vad.onnx` on the clips checked); the loader also accepts v5.

## 3. Speed and CPU (build container, not the target machine)

Container: 4 vCPU Intel Xeon @ 2.1 GHz. These numbers are slower than the M1 Max; they show the shape.

`tyst-cli live --simulate fleurs-mixed-01.wav` (57 s, 73 % speech, real-time playback, 31 partials and
8 finals). "Released" is the downloaded encoder, "banded" the rewritten one Tyst now uses (below):

| encoder | threads | partial latency p95 | end of speech → final p95 | CPU (% of one core) |
|---|---|---|---|---|
| released | 4 | 0.65 s | 1.31 s | 84 % |
| released | 1 | 1.45 s | 2.12 s | 69 % |
| **banded** | 4 | **0.31 s** | **0.87 s** | 25 % |
| **banded** | 1 | 0.53 s | 1.07 s | **19 %** |

`tyst-cli bench` on the 55 FLEURS clips: decode RTF 0.093 → 0.043 (4 threads), the same 55
transcripts word for word, RSS after load 1.30 → 1.18 GB, model load 7.0 → 2.7 s.

### The encoder's fixed cost, and the fix

With the released export, encoder time barely depends on segment length (1 thread, `onnxruntime`):

| audio | 0.5 s | 1 s | 2 s | 4 s | 8 s | 15 s | 25 s | 40 s |
|---|---|---|---|---|---|---|---|---|
| released | 0.92 s | 0.77 s | 0.93 s | 0.87 s | 0.94 s | 1.12 s | 2.43 s | 2.93 s |
| banded | 0.09 s | 0.10 s | 0.13 s | 0.20 s | 0.36 s | 0.57 s | 1.09 s | 1.97 s |
| max difference in encoder output | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Pianissimo uses block-local attention with a 256-frame window (the model card's
"att_context [256, 256]"), and the export pads every input to 256-frame blocks and scores each block
against 768 keys. A 1 s segment (13 encoder frames) cost the same attention work as a 20 s one, and
the ADR 0004 partial schedule calls the encoder about 5 times per 4 s turn.

The same attention is a T × T product with a |q − k| ≤ 256 band mask. `tyst_core::encoder_rewrite`
rewrites each of the 24 attention blocks that way and keeps every quantized projection, the
relative-position table and the masking constants as they are. The encoder output is bit-identical
to the released graph for segments both shorter and longer than the window (table above; the model
test `banded_encoder_matches_the_original` checks transcripts and token timings). The graph shrinks
from 11,643 to 3,978 nodes.

`tyst-cli models fetch` runs the rewrite after downloading (about 5 s, no network) and checks the
result against a SHA-256 pinned in `models/models.toml` (`derived`), so every machine runs the same
bytes. The model id in transcripts stays `pianissimo-sv-int8@63730c6`, since the weights and outputs
are the released ones. Both encoders stay on disk (the released one is the source); dropping it
after the rewrite would save 630 MB and is left for later.

## 4. Acceptance run on the owner's machine

AMD Ryzen 7 7800X3D (8 cores / 16 threads), Arch Linux, PipeWire microphone via ALSA, banded encoder.
The SPEC names the M1 Max; the owner ran acceptance on this machine instead.

### WER: within 1 pp of Phase 0 in every category

`tyst-cli bench eval/manifest.toml --baseline <Phase 0 summary.json>`, 95 clips (owner clips, synthetic
clips and FLEURS), 4 threads, decode RTF 0.018:

| category | clips | Phase 0 WER | `tyst-cli` WER | Δ pp |
|---|---|---|---|---|
| all | 95 | 10.7 | 10.7 | +0.0 |
| sv (owner) | 5 | 6.2 | 6.9 | +0.7 |
| en (owner) | 5 | 5.7 | 5.9 | +0.2 |
| mixed (owner) | 5 | 9.0 | 8.6 | −0.4 |
| sv-terms (owner) | 5 | 7.8 | 7.7 | −0.1 |
| synth-sv / -en / -mixed / -terms | 5 each | 8.3 / 4.2 / 4.4 / 26.1 | 8.8 / 4.2 / 4.4 / 25.4 | +0.5 / 0 / 0 / −0.7 |
| fleurs-sv / -en / -mixed | 25 / 25 / 5 | 10.4 / 23.3 / 26.0 | 10.4 / 22.9 / 25.8 | 0 / −0.4 / −0.2 |

The largest deviation is +0.7 pp on 5 Swedish clips, a handful of words. With vocabulary rules, term
recall is 62 % and exact spelling 62 % overall (sv-terms: 76 %).

### Live: partials 0.26 s p95, CPU 14–20 % of one core

`tyst-cli live --mic`, about 75 s of speech each:

| threads | partial latency p95 | end of speech → final p95 | final decode p95 | CPU (% of one core) | peak RSS |
|---|---|---|---|---|---|
| 1 | 0.55 s | 1.43 s | 0.66 s | 14 % | 1.39 GB |
| 4 | 0.26 s | 2.22 s | 0.23 s | 20 % | 1.41 GB |

All three Phase 1 targets are met on this machine: WER within 1 pp, partials well under 1.5 s and CPU
under 30 % of a core. Model load takes 1.0 s.

The end-of-speech-to-final column overstates the delay. Both runs had near-continuous speech (66 and
76 s of speech, only 6 finals each), so most finals came from the 15 s force-cut. A force-cut segment
ends at its cut point, up to 2 s before the 15 s mark that triggers it, and the live command counted
that gap as latency. The final decode itself takes 0.2–0.7 s. `live` now leaves force-cut finals
out of this number and reports how many there were. With pauses between sentences (the FLEURS clip in
section 3), p95 is 1.0 s at 4 threads, which includes the 0.5 s end-of-speech silence. The SPEC's
"final text ≤ 1.0 s after end of speech" target (section 6) is not a Phase 1 acceptance item. It
becomes one for dictation in Phase 2.
