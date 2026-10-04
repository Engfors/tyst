# Phase 0 evaluation harness

Python harness for the Phase 0 decision gate (SPEC section 12): which inference backend, which
language-routing strategy, which LID model and which segmentation parameters.

It uses **sherpa-onnx**, the same library the Rust app will call through its C API, so numbers
measured here should carry over to Phase 1. A model in onnx-asr layout is decoded with `onnx-asr`
instead (the fallback in SPEC 4.1).

No audio, transcripts or results are committed. Clips live under `clips_root` (default
`~/tyst-eval/clips`), models under `$TYST_MODELS` (default `~/.cache/tyst-eval/models`), results
under `eval/results/` (git-ignored).

## Setup

```sh
cd eval
python3 -m venv .venv && source .venv/bin/activate
python3 -m pip install -r requirements.txt
python3 -m tyst_eval fetch-models          # Pianissimo, Parakeet v3, Silero VAD, Whisper tiny/base, Piper voices
python3 -m tyst_eval fetch-fleurs          # 25 sv + 25 en FLEURS test utterances + 5 mixed concatenations
python3 -m tyst_eval synth                 # optional: 20 synthetic smoke-test clips (Piper TTS)
```

Pianissimo and FLEURS come from Hugging Face; everything else from sherpa-onnx's GitHub releases.

## Owner clips

Put 20 clips and hand-corrected references into `~/tyst-eval/clips/owner/` with the names that
[`manifest.toml`](manifest.toml) lists (`sv-01.wav` + `sv-01.txt`, …, `sv-terms-05.wav`). Any
format libsndfile reads works; edit the `audio` path if yours are `.m4a`/`.flac`. For mixed clips,
uncomment `lang_spans` and give rough start/end seconds per language block. Clips that switch
language every turn get one span per turn; a long list is fine. Add the technical
terms that occur in your clips to [`terms.toml`](terms.toml).

## Run

```sh
python3 -m tyst_eval run                                   # all clips, all strategies, greedy + hotwords
python3 -m tyst_eval run --categories sv,en,mixed,sv-terms # owner clips only
python3 -m tyst_eval run --lid whisper-base                # compare LID models
python3 -m tyst_eval run --provider coreml                 # CoreML execution provider (macOS)
python3 -m tyst_eval bench                                 # RTF + RSS per model, 1 and 4 threads, plus all models loaded together
python3 -m unittest discover -s tests                      # harness unit tests (no audio, models or network)
```

`fetch-models` refuses a download that does not match its SHA-256: archives against
`eval/models.toml`, Hugging Face snapshots against the app's `models/models.toml`.

`run` writes `eval/results/<timestamp>/`:

| file | content | shareable |
|---|---|---|
| `summary.md`, `summary.json` | metrics only: WER, term accuracy, wrong-engine rate, RTF, RSS | yes |
| `segments.jsonl`, `hyp/` | per-segment and per-clip transcripts | no, contains meeting text |

## What is measured

- **Engines**: each VAD segment is decoded once by every engine and decode mode (`greedy`,
  `beam`, `hotwords`). Hotwords use sherpa-onnx contextual biasing with the `terms` list.
- **Strategies** (SPEC 6.3), scored over the same decodes: `only-sv`, `only-en` (baselines),
  `lid` (Whisper LID per segment), `dual` (both engines, pick by mean token log-prob with a bias
  toward the current language), `parakeet-first` (Parakeet, re-decode with Pianissimo if the text
  looks Swedish). All share the hysteresis rule: segments under 1.5 s keep the current language.
- **post**: `raw` model output, or `vocab` with replacement rules and canonical term spelling.
- **WER** on normalized text (lowercase, no punctuation, digits spelled out in the clip's language so
  "18" and "arton" match; `--no-number-norm` turns that off). **Term recall**: share of term
  occurrences in the reference found in the output; **term exact** also requires exact casing.
- **Wrong-engine rate**: share of segments routed to the other language's engine, using the clip
  language or `lang_spans` at the segment midpoint.
- **RTF**: decode (+ LID) wall time divided by speech duration; `bench` also reports load time and RSS.

Segmentation parameters (`--min-silence`, `--max-speech`) and router parameters
(`--short-segment`, `--dual-bias`, `--initial-lang`) are flags so they can be tuned.
