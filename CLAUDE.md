# CLAUDE.md

Guidance for Claude Code in this repository. The full specification is [SPEC.md](SPEC.md); binding
decisions are in [docs/decisions/](docs/decisions/).

## Rules (SPEC section 0)

- Everything in English: code, comments, docs, commits, PRs, file names, even when prompted in Swedish.
- Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`), small and focused.
- Work in phases (SPEC 12). Don't start a phase before the previous one is accepted by the owner.
- **Local-only.** No network calls from `tyst-core`, ever. The only network uses in the whole app are model
  download (explicit, `tyst-cli models fetch`) and the update check (Phase 5).
- **Never log transcript text or audio**, not even at debug level. Logs carry timings, sizes, model names, error codes.
- Ask the owner, don't guess, on anything in SPEC 15 (open questions).
- Phase 0 ADRs are binding: ONNX Runtime via `ort` with our own TDT decoder (no sherpa-onnx), fixed Pianissimo
  routing with Parakeet v3 only for forced English, no language-ID model, ADR 0004 segmentation.

## Layout

```
crates/tyst-core      pipeline: resample -> Silero VAD -> segmenter -> router -> TDT engine -> vocabulary
                      -> transcript -> Markdown writer / journal. Pure Rust + ONNX Runtime, no network, no OS APIs.
crates/tyst-platform  OS integrations behind traits (Phase 1: microphone via cpal, feature `mic`).
crates/tyst-cli       headless CLI: transcribe, live, bench, models, recover.
models/models.toml    pinned model manifest (Hugging Face repo + revision + SHA-256 per file, plus files
                      `models fetch` derives locally, e.g. the banded encoder from `encoder_rewrite`).
eval/                 Phase 0 Python harness and the clip manifest (no audio in git).
docs/                 ADRs and phase reports.
```

## Commands

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cargo build --release -p tyst-cli
./target/release/tyst-cli models fetch                    # Pianissimo + Silero VAD (~660 MB)
./target/release/tyst-cli transcribe clip.wav
./target/release/tyst-cli live --mic                      # Enter stops; prints latency and CPU stats
./target/release/tyst-cli live --simulate clip.wav        # same, with a file played in real time
./target/release/tyst-cli bench eval/manifest.toml --baseline <harness summary.json>
```

Model tests in `crates/tyst-core/tests/models.rs` run only when `TYST_MODELS` points at installed models.
On Linux, building the `mic` feature needs `libasound2-dev`; `--no-default-features` builds without it.
