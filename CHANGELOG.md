# Changelog

All notable changes to Tyst. Versions follow [Semantic Versioning](https://semver.org/). The section
for a version is the text of its GitHub release and of the update notice in Settings › Updates.

## Unreleased

Features
- Custom vocabulary now helps recognition, not only spelling: terms, and replacements that join
  words (`terra form → Terraform`), make the Swedish model more likely to hear them as written.
  It is on by default, with an on/off switch and a per-rule "listen" box in Settings ›
  Vocabulary. Existing installs download one small file (360 KB) from there first. Forced English
  is not boosted yet.
- `tyst-cli bench --boost 0.25,0.5,1` scores boosted decoding next to the plain rows.

Fixes
- Finishing onboarding again after the settings could not be saved now starts meeting detection,
  the update check and idle model unload.

## 0.1.1 - 2026-10-06

The update check no longer needs a GitHub token now that the repository is public, release
signing runs only on tag builds, and fixes from the first review of 0.1.0.

Security
- The update check is anonymous now that the repository is public. The GitHub token field is gone
  from Settings › Updates, and a token saved by 0.1.0 is deleted from the keychain on first start.
- Releases are signed only from tag builds; the signing key is kept out of pull request dry runs.
- "Open transcripts folder" creates a missing transcripts folder readable by you only. A folder
  0.1.0 already created keeps its permissions; `chmod 700` it to tighten them.

Fixes
- Meeting detection, the update check and idle model unload now start as soon as onboarding
  finishes, instead of after the next restart.
- Dictation keeps a bounded audio queue, so a slow computer drops audio instead of using more and
  more memory.

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
- On macOS, Others hears only the meeting apps listed in Settings › Meetings, so music or a video
  in another app stays out of the transcript. On by default; turn it off to record all system audio.
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
- macOS 14.4 or newer: an ad-hoc signed DMG (Apple silicon), tested on an M1 Max. Allow it under
  Privacy & Security › Open Anyway on first start.

Also included
- `tyst-cli` (build from source) for transcribing files, live and meeting sessions without the app,
  benchmarks and models.
- An update notice in the tray and Settings › Updates. It never downloads or installs anything.
- Downloads come with SHA256SUMS signed by the Tyst release key (`tyst-release-key.asc`).
