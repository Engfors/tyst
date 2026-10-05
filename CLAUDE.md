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
crates/tyst-platform  OS integrations behind traits: microphone via cpal (`mic`), PipeWire mic + sink monitor
                      (`pipewire`, Linux), Core Audio process tap (`macos-tap`, macOS 14.4+).
crates/tyst-runtime   shared by app and CLI: engine loading, model fetch, two-channel `Meeting` session.
crates/tyst-cli       headless CLI: transcribe, live, meeting, bench, models, recover.
app/src-tauri         Tauri v2 app (binary `tyst`): tray, meeting window, settings, onboarding, KWin rule.
app/ui                Svelte 5 + Vite frontend, one page per window.
models/models.toml    pinned model manifest (Hugging Face repo + revision + SHA-256 per file, plus files
                      `models fetch` derives locally, e.g. the banded encoder from `encoder_rewrite`).
eval/                 Phase 0 Python harness and the clip manifest (no audio in git).
docs/                 ADRs and phase reports.
packaging/            Linux desktop file and AppImage scripts, third-party notices generator.
```

## Commands

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cargo build --release -p tyst-cli
./target/release/tyst-cli models fetch                    # Pianissimo + Silero VAD (~660 MB)
./target/release/tyst-cli transcribe clip.wav
./target/release/tyst-cli live --mic                      # Enter stops; prints latency and CPU stats
./target/release/tyst-cli live --simulate clip.wav        # same, with a file played in real time
./target/release/tyst-cli meeting --mic --system          # two-channel meeting, no UI, echo cancelled
./target/release/tyst-cli echo-cancel --mic me.wav --system others.wav --out clean.wav   # offline AEC
./target/release/tyst-cli check-update                    # GitHub latest release ($GITHUB_TOKEN)
(cd app/ui && npm ci && npm run check && npm run tauri build -- --no-bundle)   # app -> target/release/tyst
packaging/linux/fetch-tools.sh                            # pinned, hash-checked AppImage tools
(cd app/ui && NO_STRIP=true npm run tauri build -- --bundles appimage)          # Linux AppImage (CI artifact too)
packaging/linux/finish-appimage.sh target/release/bundle/appimage/*.AppImage   # go-w repack (+ update info, signing)
packaging/linux/smoke-test-appimage.sh target/release/bundle/appimage/*.AppImage
packaging/notices/generate.sh                             # THIRD_PARTY_NOTICES.md (needs cargo-about)
./target/release/tyst-cli bench eval/manifest.toml --baseline <harness summary.json>
```

`tyst-runtime`'s default `aec` feature builds WebRTC's echo canceller from source: it needs `meson`, `ninja` and
a C++ compiler. A tag `vX.Y.Z` runs `.github/workflows/release.yml` (draft release, GPG-signed checksums).
Actions are pinned to commit SHAs; keep them pinned when updating. `cargo audit` runs in CI (`.cargo/audit.toml`).

Model tests in `crates/tyst-core/tests/models.rs` are `#[ignore]`d; run them with
`TYST_MODELS=<dir> cargo test -p tyst-core --test models -- --ignored`. Eval harness tests:
`(cd eval && python3 -m unittest discover -s tests)`.
On Linux, building the `mic` feature needs ALSA headers and `pipewire` needs PipeWire headers plus clang;
the app also needs webkit2gtk-4.1 and libayatana-appindicator (see `.github/workflows/ci.yml`).
`--no-default-features` builds the CLI without audio capture.
