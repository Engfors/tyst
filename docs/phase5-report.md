# Phase 5 report: polish and release

- **Status:** Built; waiting for the owner's acceptance on Arch Linux / KDE Plasma (Wayland). macOS
  items compile and are checked in CI, and wait for the owner's Mac.
- **Code:** `crates/tyst-runtime/src/{echo,update,meeting}.rs`, `crates/tyst-platform/src/mic_watch.rs`,
  `crates/tyst-cli/src/echo_cmd.rs`, `app/src-tauri/src/{detect,secrets,updates}.rs` and the
  Settings, pill and onboarding pages; `.github/workflows/{ci,release}.yml`, `packaging/`,
  `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD_PARTY_NOTICES.md`.

## 1. Scope

| SPEC 12 Phase 5 item | Where | State |
|---|---|---|
| Echo cancellation (SPEC 6.4) | `tyst_runtime::echo`, wired into `Meeting` | Built, on by default |
| Opt-in meeting auto-detection (SPEC 8.3, 9.4) | `tyst_platform::mic_watch`, `detect.rs`, pill prompt | Built, off by default |
| Update notification (SPEC 9.7) | `tyst_runtime::update`, `updates.rs`, tray, Settings › Updates | Built |
| macOS "only meeting apps" filter (SPEC 15 q7) | — | Deferred until the owner is on the Mac |
| README with screenshots | `README.md`, `docs/screenshots/` | Built |
| THIRD_PARTY_NOTICES and About attributions (SPEC 13) | `packaging/notices/`, Settings › About | Built |
| Release workflow: macOS .app/.dmg and Linux AppImage on tag | `.github/workflows/release.yml` | Built; first real run on the first tag |
| Models unload when idle (SPEC 11) | `AppState::unload_when_idle`, Settings › General | Built, off by default |

Open issues closed here: #8 part 2 (the keyboard portal token moves from `config.toml` into the
keychain) and from #14 PR7-003, CI-001/PR7-011, SEC-007, PR7-005, PR7-006, PR7-009, DOC-001/PR7-010
and DOC-002.

## 2. Echo cancellation

With speakers instead of headphones, the microphone hears the others, and the Me pipeline wrote
their words a second time under "Me". Tyst now runs WebRTC's AEC3 (`webrtc-audio-processing` 2.1,
built from source with meson) on the Me channel at 16 kHz in 10 ms frames, with the Others capture as
the far-end reference.

- **Alignment.** Both channels carry session-clock positions. The Others capture feeds an
  `EchoReference` (3 s ring buffer, gaps zero-filled) from a tee thread *before* its own decoder, so
  a slow decode on Others never starves the canceller. Me waits up to 250 ms for the reference to
  cover a block; if Others has stopped delivering (no system audio), Me goes on without it.
- **Residual gate.** AEC3 leaves a quiet residual of loud far-end speech that Pianissimo still
  transcribed as stray words. When AEC3 removed more than 12 dB of a frame's energy (output under −12 dB
  relative to input), the frame is mostly echo and is attenuated by another 18 dB, with smoothing
  so near-end speech during double talk is not chopped.
- **Measured.** Synthetic test signals in `echo.rs`: the system audio is reduced by more than 20 dB,
  and the user talking over the others keeps their level within 6 dB. On FLEURS Swedish and English
  clips mixed with an 80 ms, 0.4-gain echo with a reverb tail and noise, the echo regions drop by
  15 to 50 dB, and a live `Meeting` run over the same clips wrote none of the Others sentences under
  Me (before: every sentence twice).
- **Tools.** `tyst-cli meeting --no-echo-cancellation` and Settings › Meetings turn it off;
  `tyst-cli echo-cancel --mic --system --out` runs the canceller over two recordings offline.

## 3. Meeting detection

Off by default. When on, Tyst looks every 2 seconds at which apps hold a capture stream: on Linux
the PipeWire registry (`Stream/Input/Audio` nodes joined with their clients'
`application.name` and `application.process.binary`), on macOS Core Audio's process objects
(`kAudioProcessPropertyIsRunningInput`, bundle id). An app matching the list in Settings
(substring, case-insensitive; Teams, Zoom, Webex, Slack, Discord, Skype, Jitsi and the common
browsers by default) makes the pill ask "Firefox is using the microphone. Transcribe this
meeting?" for 20 seconds. Nothing is recorded without "Start transcribing". Each microphone use is
asked about once, and only while Tyst is idle. Tyst's own streams are left out.

## 4. Updates

On launch (after 20 s) and every 24 hours Tyst calls the GitHub releases API for
`Engfors/tyst` and compares versions (semver). A newer release shows a blue dot on the tray
icon, an "Update available" tray item and Settings › Updates with the notes and a link to the
release page. Nothing is downloaded or installed. While the repository is private the request needs
a fine-grained token with read access to its contents; it is stored in the keychain (Secret Service
on Linux, the login keychain on macOS) and only ever sent to `api.github.com`. Settings can turn
the check off. `TYST_PRETEND_VERSION=0.0.1` makes any release look newer, for testing.

The keyboard portal's restore token (Linux paste) moved into the same keychain (#8). An existing
token in `config.toml` is moved on the first start and removed from the file; the config only
remembers whether one is stored.

## 5. Releases and CI

- **Release workflow.** A tag `vX.Y.Z` (it must match the workspace version) builds the AppImage on
  Ubuntu 24.04 and the .app and .dmg on macOS (ad-hoc signed; no Developer ID yet), writes
  `SHA256SUMS`, signs it with the release GPG key (`SHA256SUMS.asc`, plus the public key
  `tyst-release-key.asc`) and opens a **draft** release. The owner publishes it, which is when the
  update check sees it. Only the job that creates the release has `contents: write`. Running the
  workflow by hand is a dry run that builds everything without a release.
- **AppImage.** `packaging/linux/fetch-tools.sh` puts AppRun, linuxdeploy and
  linuxdeploy-plugin-appimage at pinned, SHA-256-checked versions where the Tauri bundler looks, so
  it never downloads a tool itself. The binary is built without debug info. After bundling,
  `finish-appimage.sh` repacks the image with the pinned appimagetool and the image's own runtime:
  group and world write bits are cleared, and release builds embed the update information
  (`gh-releases-zsync|Engfors|tyst|latest|Tyst_*_amd64.AppImage.zsync`, with the `.zsync` file)
  and a GPG signature. `smoke-test-appimage.sh` checks the binary's `NEEDED` libraries, that it
  is stripped, that the license files are inside, that no `.onnx` model is, and that nothing is
  group or world writable.
- **CI.** All actions are pinned to commit SHAs, the workflow token is read-only, `cargo audit`
  runs on every change (`.cargo/audit.toml` ignores RUSTSEC-2024-0429 and RUSTSEC-2024-0370, both in
  Tauri's GTK 3 stack, with the reason), and the eval harness's Python tests run when present.

## 6. Licensing

Tyst is MIT OR Apache-2.0. `THIRD_PARTY_NOTICES.md` (cargo-about for the Rust crates plus a written
part for the models, ONNX Runtime, WebRTC and Abseil, the AppImage's bundled libraries and the UI)
ships inside the app and opens from Settings › About, which also states that models are downloaded
only when the user asks.

## 7. Open

- macOS: the "only meeting apps" system-audio filter (SPEC 15 q7), and an owner test of echo
  cancellation, detection and the DMG.
- The first real release needs the `RELEASE_GPG_KEY` and `RELEASE_GPG_PASSPHRASE` secrets.
