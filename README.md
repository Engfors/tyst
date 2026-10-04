# Tyst

Local, privacy-first meeting transcription and dictation for macOS and Linux. Swedish and English,
including mixed-language meetings. Nothing leaves the device.

Status: **Phase 4** (Linux AppImage). See [SPEC.md](SPEC.md) for the specification,
[docs/phase4-report.md](docs/phase4-report.md) for the AppImage, [docs/phase3-report.md](docs/phase3-report.md)
for dictation, [docs/phase2-report.md](docs/phase2-report.md) for the meeting app,
[docs/phase1-report.md](docs/phase1-report.md) for pipeline measurements and
[docs/phase0-report.md](docs/phase0-report.md) for the model evaluation.

## Try it

```sh
cargo build --release -p tyst-cli
./target/release/tyst-cli models fetch          # Klang Pianissimo int8 + Silero VAD, verified by SHA-256
./target/release/tyst-cli transcribe meeting.wav --out-dir ~/Transcripts --title "Weekly sync"
./target/release/tyst-cli live --mic            # live partials and finals; Enter stops and prints latency/CPU
```

## Meeting app

```sh
cd app/ui && npm ci && npm run tauri build -- --no-bundle   # or `npm run tauri dev`
../../target/release/tyst                                    # tray icon; onboarding on first run
../../target/release/tyst --toggle-meeting                   # start/stop from a shortcut
```

The app records your microphone (Me) and system audio (Others) as two pipelines, shows a floating
meeting window that never takes focus, and saves Markdown when you stop. On Linux it captures through
PipeWire; building it needs `webkit2gtk-4.1`, `libayatana-appindicator`, `librsvg`, `pipewire` and
`clang`. `tyst-cli meeting --mic --system` runs the same session without the UI.

### AppImage (Linux)

```sh
(cd app/ui && npm ci && NO_STRIP=true npm run tauri build -- --bundles appimage)
install -Dm755 target/release/bundle/appimage/Tyst_*_amd64.AppImage ~/.local/share/AppImage/Tyst.AppImage
~/.local/share/AppImage/Tyst.AppImage
```

CI builds the same AppImage on Ubuntu 24.04 (artifact `tyst-appimage`). `NO_STRIP=true` is needed on
Arch, whose libraries use ELF sections linuxdeploy's `strip` does not know. The AppImage needs
`fusermount3` (`fuse3`). Models are not bundled.

Models live outside the repo: `~/Library/Application Support/Tyst/models` on macOS,
`~/.local/share/tyst/models` on Linux, or `$TYST_MODELS`. After downloading, `models fetch` also writes
`encoder-model.banded.int8.onnx`: the same Pianissimo encoder with its local attention computed without
256-frame block padding (same outputs, much less work for short segments; `tyst_core::encoder_rewrite`).

## Attribution

- [Klang Pianissimo](https://huggingface.co/KlangAI/pianissimo-sv) © Klang AI AB, CC BY 4.0. Tyst runs
  a locally rewritten copy of its int8 encoder (attention graph only; weights unchanged).
- `crates/tyst-core/proto/onnx.proto` from [ONNX](https://github.com/onnx/onnx) v1.17.0, Apache-2.0.
- NVIDIA Parakeet TDT 0.6B v3 © NVIDIA, CC BY 4.0 (only for forced English).
- Silero VAD, MIT. ONNX Runtime, MIT.
