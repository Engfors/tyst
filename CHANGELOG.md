# Changelog

All notable changes to Tyst. Versions follow [Semantic Versioning](https://semver.org/). The section
for a version is the text of its GitHub release and of the update notice in Settings › Updates.

## 0.1.0 - 2026-10-05

The first release: local meeting transcription and dictation in Swedish and English, including
mixed-language speech. Nothing leaves the computer except the model download and, if left on, the
daily update check.

Meetings
- Records your microphone (Me) and the system audio (Others) as two channels, shows the text live
  in a small floating window and saves a Markdown transcript when you stop.
- Echo cancellation (WebRTC AEC3) on the Me channel, so with speakers the others are not written
  twice. Turn it off in Settings › Meetings.
- Optional meeting detection: when Teams, Zoom, a browser or another listed app starts using the
  microphone, Tyst asks whether to transcribe. Off by default; it never starts on its own.
- A journal survives crashes: after an interrupted meeting, Tyst offers to recover it on the next
  start (`tyst-cli recover` does the same from the command line).

Dictation
- Hold or tap the dictation shortcut, speak, and the text is pasted where you were typing.
- A custom vocabulary with replacement rules for names and terms (Settings › Vocabulary).

Speech recognition
- Klang Pianissimo (Swedish and English) on ONNX Runtime with Silero VAD, with Parakeet v3 only when
  English is forced. Models (about 660 MB) are downloaded once and checked by SHA-256.
- Models can unload after a few idle minutes to free about 1 GB (Settings › General).

Platforms
- Linux: a signed AppImage (x86_64, glibc 2.39 or newer), tested on Arch with KDE Plasma (Wayland)
  and PipeWire.
- macOS 14.4 or newer: an ad-hoc signed DMG (Apple silicon). Not yet tested on a real Mac; right-click
  and Open on first start.

Also included
- `tyst-cli` for transcribing files, live and meeting sessions without the app, benchmarks and models.
- An update notice in the tray and Settings › Updates. It never downloads or installs anything.
- Downloads come with SHA256SUMS signed by the Tyst release key (`tyst-release-key.asc`).
