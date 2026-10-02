# Tyst

Local, privacy-first meeting transcription and dictation for macOS and Linux. Swedish and English,
including mixed-language meetings. Nothing leaves the device.

Status: **Phase 1** (core pipeline + CLI). See [SPEC.md](SPEC.md) for the specification,
[docs/phase1-report.md](docs/phase1-report.md) for current measurements and
[docs/phase0-report.md](docs/phase0-report.md) for the model evaluation.

## Try it

```sh
cargo build --release -p tyst-cli
./target/release/tyst-cli models fetch          # Klang Pianissimo int8 + Silero VAD, verified by SHA-256
./target/release/tyst-cli transcribe meeting.wav --out-dir ~/Transcripts --title "Weekly sync"
./target/release/tyst-cli live --mic            # live partials and finals; Enter stops and prints latency/CPU
```

Models live outside the repo: `~/Library/Application Support/Tyst/models` on macOS,
`~/.local/share/tyst/models` on Linux, or `$TYST_MODELS`.

## Attribution

- [Klang Pianissimo](https://huggingface.co/KlangAI/pianissimo-sv) © Klang AI AB, CC BY 4.0.
- NVIDIA Parakeet TDT 0.6B v3 © NVIDIA, CC BY 4.0 (only for forced English).
- Silero VAD, MIT. ONNX Runtime, MIT.
