# Phase 1 report: core pipeline and CLI

- **Status:** In progress. Built and measured in the cloud build container; the owner-clip WER run
  and the M1 Max live run are still to do (section 4).
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
| Unit tests: segmenter, router hysteresis, vocabulary, Markdown, journal recovery | 51 tests in `tyst-core` |

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

`tyst-cli live --simulate fleurs-mixed-01.wav` (57 s, 73 % speech, real-time playback):

| threads | partial latency p95 | end of speech → final p95 | CPU (% of one core) |
|---|---|---|---|
| 4 | 0.64 s | 1.23 s | 80 % |
| 1 | 1.49 s | 2.01 s | 67 % |

### Finding: the encoder has a fixed cost of ~0.8 s per call

Encoder time barely depends on segment length (1 thread, same numbers from Python onnxruntime 1.30
and from `ort`):

| audio | 0.5 s | 1 s | 2 s | 4 s | 8 s | 15 s |
|---|---|---|---|---|---|---|
| encoder | 0.83 s | 0.83 s | 0.84 s | 1.09 s | 1.21 s | 1.60 s |

ONNX Runtime profiling shows why: Pianissimo's export uses block-local attention with a 256-frame
window (the model card's "att_context [256, 256]"), and the exported graph pads every input to
256-frame blocks and attends over 768 keys per block. A 1 s segment (13 encoder frames) costs the
same attention work as a 20 s one. The Phase 0 harness had the same cost, which is why its RTF looked
fine on 8 s chunks but the CPU estimate was "at risk".

Consequences:

- Partials are the expensive part. The ADR 0004 schedule calls the encoder 5 times for a 4 s turn
  (4 partials + final), about 4 s of single-core time in this container for 4 s of speech.
- Latency targets hold here with 4 threads; the 30 %-of-a-core CPU target does not, in this container.
- Every segment is at most 15 s (188 frames), shorter than the 256-frame window, so for our inputs
  local attention gives the same result as full attention. An encoder export without the 256-frame
  padding (full attention, or the same graph with a smaller block size chosen per input length)
  would be numerically equivalent and remove most of the fixed cost. That needs graph surgery on the
  int8 export or a re-export from the `.nemo` checkpoint, plus a parity check like section 2. Not done
  in this PR.

Memory: 1.30 GB RSS after loading Pianissimo, 1.5 GB peak during live use (Phase 0 harness: 1.33 GB
after load on the same machine). Model load takes 3–8 s here (the harness took 8.6 s on the same machine).

## 4. Still open for Phase 1 acceptance

1. **Owner clips, WER within 1 pp** (owner machine):
   `tyst-cli bench eval/manifest.toml --baseline <Phase 0 summary.json>`. The `Δ pp` column is the check.
2. **Live partial latency ≤ 1.5 s on the M1 Max:** `tyst-cli live --mic` (Enter stops) prints
   partial latency, end-of-speech-to-final latency and CPU. `--threads 1` and `--threads 4` both matter
   for the CPU target.
3. **CPU target:** decide whether to remove the encoder's fixed cost (section 3) now or after the
   M1 Max numbers are in.
