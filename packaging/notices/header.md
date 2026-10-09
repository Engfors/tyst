# Third-party notices

Tyst is © 2026 Emil Engfors and the Tyst contributors, licensed under MIT OR Apache-2.0 (see
`LICENSE-MIT` and `LICENSE-APACHE`). It is built on the work below. This file ships inside the app
(Settings › About › Third-party notices) and is regenerated with `packaging/notices/generate.sh`.

## Speech models

The models are not part of the app or this repository. `tyst-cli models fetch` or the app's
onboarding downloads them from Hugging Face only when you ask, and checks each file's SHA-256
against `models/models.toml`.

- **Klang Pianissimo** (`KlangAI/pianissimo-sv`, ONNX export `KlangAI/pianissimo-sv-onnx`), © Klang AI AB, licensed under
  [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Tyst runs it unchanged, plus a copy of
  its int8 encoder that `models fetch` rewrites locally: the local-attention graph is computed
  without 256-frame block padding; the weights are unchanged.
- **NVIDIA Parakeet TDT 0.6B v3** (`nvidia/parakeet-tdt-0.6b-v3`, ONNX export
  `istupakov/parakeet-tdt-0.6b-v3-onnx`), © NVIDIA, licensed under
  [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Only used when English is forced.
- **Silero VAD** (`snakers4/silero-vad`, downloaded from `csukuangfj/vad`), © Silero Team, MIT License.

## Native libraries

- **ONNX Runtime** (Microsoft), MIT License. Linked into Tyst through the `ort` crate.
  Copyright (c) Microsoft Corporation.
- **WebRTC audio processing** (echo cancellation, AEC3), © 2011 Google Inc., BSD-3-Clause; built
  from the `webrtc-audio-processing` package by PulseAudio/freedesktop.org, with its text below
  under the `webrtc-audio-processing` crates.
- **Abseil C++** (`abseil-cpp` 20240722.0, built into the echo canceller), © The Abseil Authors,
  Apache-2.0.
- **`crates/tyst-core/proto/onnx.proto`** from [ONNX](https://github.com/onnx/onnx) v1.17.0, © ONNX
  Project Contributors, Apache-2.0.
- **`crates/tyst-core/proto/sentencepiece_model.proto`** from
  [SentencePiece](https://github.com/google/sentencepiece) v0.2.0, © Google Inc., Apache-2.0.
  `crates/tyst-core/src/asr/spm.rs` ports its BPE encoder and normalizer.
- **Phrase boosting** (`crates/tyst-core/src/asr/boost.rs`) is a port of Klang AI's
  `phrase_boost.py` (`KlangAI/pianissimo-sv-onnx`, CC BY 4.0), itself a port of NVIDIA NeMo's GPU-PB
  boosting tree (NeMo 2.7, Apache-2.0).

## Linux AppImage

The AppImage bundles the system libraries the app needs from Ubuntu 24.04, among them WebKitGTK
and JavaScriptCore (LGPL-2.1 and BSD), GTK 3, GLib, Cairo and Pango (LGPL-2.1-or-later), libsoup
(LGPL-2.0-or-later) and their dependencies, each under its own license. The image carries them
unmodified; their sources are available from Ubuntu
(`apt-get source <package>`, see <https://packages.ubuntu.com/noble/>). The AppImage runtime and
AppRun come from the AppImage project and Tauri (MIT).

## User interface

The UI bundle contains code from **Svelte** (MIT, © Svelte contributors) and **@tauri-apps/api**
(MIT OR Apache-2.0, © Tauri Programme within The Commons Conservancy).

## Licenses with notable terms

- Crates under **MPL-2.0** (Symphonia audio decoders, `cssparser`, `selectors`, `dtoa-short`,
  `option-ext`) are used unmodified; their source is at the links below.
- **CDLA-Permissive-2.0** covers the Mozilla root certificate data in `webpki-roots`, used for
  the HTTPS connections of the model download and the update check.

