# SPEC — Tyst: local meeting transcription & dictation

> **Working name:** `Tyst` (Swedish for "quiet" — nothing leaves the device). Final name TBD — rename with a project-wide find/replace (`Tyst` / `tyst`).
> **Status:** Draft v1 — 2026-10-02
> **Owner:** Emil Engfors
> **Audience:** Claude Code (implementation) and the owner (review).

---

## 0. Instructions for Claude Code

- **Language:** All code, comments, docs, commit messages, PR titles/descriptions, issue text and file names are in **English**, even when the owner prompts in Swedish.
- **Commits:** Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`). Small, focused commits.
- **Work in phases** (section 12). Do not start a phase before the previous phase's acceptance criteria are met and the owner has confirmed. Phase 0 decisions (model routing, inference backend) are binding for later phases; record them in `docs/decisions/` as short ADRs.
- **Local-only is a hard rule.** No network calls from the core crate, ever. The only permitted network access in the whole app: (1) model download on first run / on demand, (2) the update check (section 9.7). Both are user-visible and can be disabled.
- **Never log transcript text or audio** — not even at debug level. Logs may contain timings, sizes, model names and error codes.
- **Ask, don't guess,** on anything listed in section 15 (Open questions).
- Suggest creating a `CLAUDE.md` from this section plus the repo layout once Phase 1 scaffolding exists.

---

## 1. Summary

A small, fast, privacy-first desktop app that:

1. **Transcribes meetings live** (Teams, Google Meet, Zoom — any app) by capturing the computer's **microphone ("Me")** and **system audio ("Others")** as two separate channels, shows the text live in a small floating window, and saves the result as a **Markdown file** in a user-chosen folder.
2. **Dictates** into any app or terminal via a global shortcut (default **Cmd+Å** on macOS / **Ctrl+Å** on Linux), showing the text in a small **pill** at the bottom of the screen before pasting.
3. Handles **Swedish and English, including mixed-language meetings** (language switches between speakers and English words inside Swedish sentences).
4. Runs **100 % on-device**. No cloud transcription.

Primary ASR model: **Klang Pianissimo** (`KlangAI/pianissimo-sv`, a Swedish fine-tune of NVIDIA Parakeet TDT 0.6B v3). Because Pianissimo's English and code-switching performance is officially *not established*, the app uses a **multi-model routing design** that is validated in Phase 0.

---

## 2. Goals and non-goals

### Goals (v1)
- G1. Live meeting transcription with "Me" / "Others" separation, saved as Markdown.
- G2. System-wide dictation with preview-then-paste (or optional direct paste), working in GUI apps and terminals.
- G3. Swedish + English + mixed-language support with good accuracy for technical vocabulary (custom vocabulary list).
- G4. Minimal, Spotlight-inspired UI: floating meeting window + dictation pill + menu bar / system tray icon. No main window, no Dock icon.
- G5. **macOS first** (Apple Silicon, primary work machine), then **Arch Linux / KDE Plasma on Wayland**.
- G6. One shared codebase; platform differences isolated in thin modules.
- G7. Fully local inference; no telemetry.

### Non-goals (v1) — see section 14 for future ideas
- iOS / iPhone (covered by the third-party app SnickSnack for now).
- Windows.
- Summaries, action items or any LLM post-processing.
- Speaker diarization beyond Me vs Others.
- Saving audio files.
- Per-turn timestamps in the Markdown output (future feature).
- Signed/notarized macOS builds (future).
- Auto-installing updates (v1 only notifies).

---

## 3. Target platforms and hardware

| | macOS (priority 1) | Arch Linux (priority 2) |
|---|---|---|
| OS | macOS 14.4+ (required for Core Audio process taps) | Rolling Arch, KDE Plasma 6, **Wayland** session |
| Owner's hardware | MacBook M1 Max, 32 GB RAM | Ryzen 7 7800X3D, NVIDIA RTX 4090 |
| Inference | CPU (ONNX Runtime). CoreML execution provider optional, evaluated in Phase 0 | CPU by default; CUDA execution provider optional |
| Distribution | Locally built `.app` (ad-hoc / self-signed); `.dmg` optional | **AppImage** |
| Tray | Menu bar extra | StatusNotifierItem (KDE system tray) |

The app must run comfortably on CPU only; GPU acceleration is a bonus, not a requirement.

---

## 4. Architecture

### 4.1 Stack decision
- **Tauri v2** app shell for both platforms (one UI codebase).
- **Rust** for everything non-UI: audio, inference, routing, file output, platform integrations.
- **Frontend:** Svelte 5 + TypeScript + Vite, no UI framework/component library. Hand-written CSS; tiny bundle.
- **Inference:** **sherpa-onnx** (via Rust bindings, e.g. `sherpa-rs`, or its C API) on top of ONNX Runtime. Fallback if Pianissimo's ONNX export is incompatible with sherpa-onnx's transducer loader: ONNX Runtime directly through the `ort` crate with a hand-written TDT greedy decoder. Decided in Phase 0.
- **Small native shims are allowed** where Rust bindings are impractical (e.g. a Swift helper for Core Audio taps via `swift-rs`, or a KWin script). Keep each shim minimal and behind a Rust trait.

### 4.2 Layers

```
┌──────────────────────────────────────────────────────┐
│ UI (Tauri webview, Svelte)                           │
│  meeting window · dictation pill · settings · onboard│
├──────────────────────────────────────────────────────┤
│ App crate (Tauri commands, events, state, tray)      │
├──────────────────────────────────────────────────────┤
│ Platform crate (trait impls per OS)                  │
│  AudioCapture · GlobalHotkey · TextInjector ·        │
│  Clipboard · WindowBehavior · MeetingDetector ·      │
│  Autostart                                           │
├──────────────────────────────────────────────────────┤
│ Core crate (pure Rust, no network, no UI, no OS APIs)│
│  resample → VAD → segmenter → language router →      │
│  ASR engine(s) → vocabulary post-proc →              │
│  transcript model → Markdown writer / journal        │
└──────────────────────────────────────────────────────┘
```

### 4.3 Repository layout

```
tyst/
├── SPEC.md
├── CLAUDE.md                  (created in Phase 1)
├── README.md
├── LICENSE                    (TBD, see §15)
├── THIRD_PARTY_NOTICES.md
├── Cargo.toml                 (workspace)
├── crates/
│   ├── tyst-core/            (pipeline, models, transcript, writer)
│   ├── tyst-platform/        (traits + macos/ + linux/ modules)
│   └── tyst-cli/             (headless CLI: benchmark, transcribe file, live mic test)
├── app/
│   ├── src-tauri/             (Tauri app crate)
│   └── ui/                    (Svelte frontend)
├── eval/                      (Phase 0 scripts, test manifests — NO audio committed)
├── docs/
│   ├── decisions/             (ADRs)
│   └── phase0-report.md
└── packaging/
    ├── macos/
    └── linux/                 (AppImage config, .desktop, icons, KWin rules/scripts)
```

### 4.4 Core traits (sketch)

```rust
trait AudioSource {            // one per channel
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}
struct AudioChunk { channel: Channel, sample_rate: u32, samples: Vec<f32>, t0: Duration }
enum Channel { Me, Others }

trait AsrEngine {
    fn id(&self) -> &str;                 // "pianissimo-sv-int8", "parakeet-v3-int8"
    fn transcribe(&self, pcm_16k_mono: &[f32]) -> Result<AsrResult>;
}
struct AsrResult { text: String, tokens: Vec<Token>, confidence: f32 }

trait LanguageRouter {
    fn route(&mut self, seg: &SpeechSegment) -> Result<RoutedResult>; // picks engine(s), returns text + lang
}

trait TextInjector { fn paste(&self, text: &str, mode: PasteMode) -> Result<()>; }
trait GlobalHotkey  { fn register(&mut self, id: HotkeyId, accel: &str, cb: Box<dyn Fn(HotkeyEvent)>) -> Result<()>; }
```

---

## 5. Models

### 5.1 Models used

| Role | Model | Notes |
|---|---|---|
| Swedish ASR (primary) | `KlangAI/pianissimo-sv-onnx` (int8) | CC BY 4.0, Klang AI AB. ~600M params. Punctuation + casing. |
| English / multilingual ASR | NVIDIA `parakeet-tdt-0.6b-v3` ONNX (int8; sherpa-onnx provides a ready export) | CC BY 4.0, NVIDIA. Same architecture and tokenizer as Pianissimo. |
| Voice activity detection | Silero VAD (ONNX) | MIT. |
| Language ID (candidate) | sherpa-onnx spoken-language-ID (Whisper tiny/base multilingual) — *or* none, see §6.3 | Decided in Phase 0. |

### 5.2 Model management
- Models are **not bundled** in the app/AppImage. They are downloaded on first run (with consent, size shown) to:
  - macOS: `~/Library/Application Support/Tyst/models/`
  - Linux: `~/.local/share/tyst/models/`
- Pinned versions with SHA-256 checksums in a manifest in the repo. Verify after download; resume partial downloads.
- Also support **"Use local model folder"** (manual import, fully offline install).
- Settings shows installed models, size, version, and a "Re-download / verify" button.
- Memory budget: both ASR models + VAD (+ LID) loaded concurrently should stay **< 3 GB RSS** with int8. Load lazily; keep warm while app runs (configurable "unload after N minutes idle").

---

## 6. Transcription pipeline

### 6.1 Audio
- Each channel is captured independently, converted to **mono f32**, resampled to **16 kHz** (`rubato`).
- Channels are processed by **independent pipelines** (own VAD, own segmenter) and merged into a single timeline by timestamp.
- **Me** = default (or chosen) microphone. **Others** = system output audio (all apps by default; optional "only meeting apps" filter on macOS via per-process taps).

### 6.2 Segmentation and live partials
Pianissimo / Parakeet are offline (non-streaming) transducers, so live text uses **VAD-segmented pseudo-streaming**:
1. Silero VAD detects speech start/end per channel.
2. While a segment is open, re-decode the growing buffer every **~800 ms** → emit `partial` event (replaces previous partial for that segment in the UI).
3. On speech end (silence ≥ ~500 ms, tunable) → final decode → emit `final` event.
4. Force-cut segments at **~15 s** (cut at the lowest-energy point in the last 2 s) to bound latency and memory; continue in a new segment.
5. Very short segments (< 300 ms) are dropped unless they contain decodable words.

Targets on M1 Max (CPU): partial latency ≤ 1.5 s, final text ≤ 1.0 s after end of speech, CPU usage during a meeting ≤ 30 % of one performance core on average.

### 6.3 Language routing (validated in Phase 0)
Two real-world patterns must work:
- **A — language changes between segments/speakers** (meeting starts in Swedish, switches to English when non-Swedish speakers join).
- **B — English words inside Swedish sentences** ("vi måste fixa vår Terraform state drift innan deploy").

Candidate strategies (implement behind `LanguageRouter`, select via config):
1. **LID-routed:** language-ID per segment → Pianissimo (sv) or Parakeet v3 (en).
2. **Dual-decode, pick best:** run both engines on each final segment, choose by confidence (mean token log-prob, length-normalized) with a bias toward the current meeting language. Partials use only the currently preferred engine.
3. **Parakeet-first routing:** decode with Parakeet v3 (which auto-detects among its languages); if the output is Swedish, re-decode with Pianissimo.

Common rules regardless of strategy:
- **Hysteresis / meeting language prior:** keep a running "current language" per channel. Short segments (< 1.5 s, e.g. "ok", "yes", "precis") inherit the current language instead of flipping it.
- Pattern B is handled by Pianissimo itself (Swedish segment, English loanwords inside) + the custom vocabulary (§9.3). Phase 0 must measure how well that works.
- User can force a language for the session (Auto / Swedish / English) from the meeting window and the pill.

### 6.4 Echo / bleed between channels
If the user is on speakers, the mic also picks up "Others", producing duplicate text attributed to "Me".
- v1: onboarding recommends headphones; detect likely bleed (high correlation between system output and mic) and show a subtle hint.
- Phase 5: acoustic echo cancellation on the Me channel using the system audio as reference (`webrtc-audio-processing` crate).

### 6.5 Transcript model

```rust
struct Segment {
    id: u64, channel: Channel, start: Duration, end: Duration,
    lang: Lang /* Sv | En */, engine: String,
    text: String, state: SegState /* Partial | Final */, edited: bool,
}
```

### 6.6 Crash safety
- While recording, final segments are appended to a **journal file** (`<transcripts>/.tyst-journal/<session-id>.jsonl`) immediately.
- On next launch, an orphaned journal triggers "Recover unsaved meeting from <time>?" → writes the Markdown file.
- Journal is deleted after a successful save.

---

## 7. Output: Markdown file

- Saved to the **transcripts folder** chosen during onboarding (changeable in settings).
- File name: `YYYY-MM-DD HHmm <title>.md`; if no title: `YYYY-MM-DD HHmm Meeting.md`. Sanitize for both filesystems; never overwrite (append ` (2)` etc.).
- Consecutive segments from the same channel are merged into one paragraph per turn.

```markdown
---
title: Customer sync – Acme
date: 2026-10-02
start: "10:15"
end: "10:52"
duration: 37m
languages: [sv, en]
app: Tyst 0.1.0
models: [pianissimo-sv-int8@<rev>, parakeet-tdt-0.6b-v3-int8@<rev>]
---

# Customer sync – Acme

**Me:** Hej allihop, vi börjar med en snabb genomgång av vår Terraform-setup.

**Others:** Låter bra. Vi har några frågor kring state drift.

**Others:** Sorry I'm late — should we switch to English?

**Me:** Sure, no problem. Let's continue in English.
```

- Speaker labels are configurable (default "Me" / "Others"; e.g. "Emil" / "Others").
- Per-turn timestamps: **not in v1** (keep the data in the journal so it can be added later as an option).

---

## 8. UX

### 8.1 Design principles
- Spotlight/Raycast-inspired: small, rounded, translucent/blurred background, system font, no chrome, dark/light follows the OS.
- **Never steal focus.** The meeting window and pill are always-on-top and *non-activating*: clicking them must not move keyboard focus away from Teams/the terminal, except when the user explicitly clicks into a text field (e.g. the title prompt).
- Keyboard-first; every action reachable without the mouse.
- Motion is subtle (≤ 150 ms), respects "reduce motion".

### 8.2 Presence
- **Menu bar icon (macOS) / system tray icon (KDE).** No Dock icon / no taskbar entry.
- Icon states: idle · meeting recording (red dot) · dictating (pulse) · paused · error.
- Tray menu: Start/Stop meeting transcription · Pause/Resume · Show/Hide meeting window · Language: Auto/Svenska/English · Open transcripts folder · Settings… · Check for updates · About · Quit.
- **Launch at login: on by default**, configurable.

### 8.3 Meeting transcription flow

**Start**
- Manually (default): tray menu, or meeting shortcut (default **Cmd+Shift+Å / Ctrl+Shift+Å**, configurable — see §15).
- Optional **auto-detect** (setting, *off* by default): when a known meeting app (Teams, Zoom, browser with mic active) starts using the microphone, the pill shows "Teams is using the microphone — start transcription?" with *Start* (Enter) / *Dismiss* (Esc). Never starts recording without user confirmation.

**During**
- Floating **meeting window** (~420×260 px default, resizable):
  - Header: recording dot + elapsed time · two tiny level meters (Me / Others) · language badge (SV/EN/Auto) · pause · stop.
  - Body: live transcript, newest at the bottom. Partial text shown dimmed/italic, replaced by final text.
  - Each turn labelled Me/Others (subtle color difference) and a small language tag when it differs from the previous turn.
  - **Scrollback:** user can scroll up freely; auto-scroll pauses while scrolled up and a "↓ Latest" chip appears to jump back.
  - **Drag anywhere on the header to move.** Position and size are remembered per display.
  - Can be collapsed to a compact one-line mode (shows only the latest line) and hidden entirely (recording continues; tray icon shows state).
- Pause stops capture on both channels; a "Paused" marker is written to the transcript.

**Stop**
- Stop via button, tray, or meeting shortcut.
- Window shows an inline prompt: **"Name this meeting"** text field (focused, placeholder = timestamp name), *Save* (Enter), and the file path preview.
- If the user presses Esc, clicks away, or ignores it for 30 s → save with the timestamp name.
- After saving: brief toast "Saved · Open · Show in folder", then the window hides.

### 8.4 Dictation flow

**Trigger:** global shortcut, default **Cmd+Å (macOS) / Ctrl+Å (Linux)**, configurable.
- Default behavior: **press to start, press again to stop**. If the key is held > 400 ms, it acts as **push-to-talk** (release = stop). Both behaviors coexist; a setting can restrict to one.
- Esc while dictating = cancel.

**Pill** (bottom-center of the active display, ~ 60 px above the Dock/panel):
- Listening: small waveform + live partial text (single line, scrolls horizontally / grows to max 2 lines).
- Done (default **preview mode**): shows final text (up to ~4 lines, scrollable) with hints: `⏎ Paste · ⌘C Copy · Esc Discard` (Linux shows Ctrl equivalents). The user can also click into the text to make quick edits before pasting.
- **Direct paste mode** (setting): text is pasted immediately when dictation stops; the pill flashes "Pasted" with an *Undo-copy* affordance (copies text to clipboard if paste landed in the wrong place).

**Paste mechanics**
- Remember the focused app/window when dictation starts; paste goes there.
- Put text on the clipboard → synthesize paste keystroke → **restore the previous clipboard** after ~500 ms (setting, on by default).
- Terminals: on Linux, detect terminal windows (window class list, default includes `com.mitchellh.ghostty`, `org.kde.konsole`, `kitty`, `alacritty`, `foot`, `wezterm`) and send **Ctrl+Shift+V** instead of Ctrl+V. macOS terminals accept Cmd+V.
- Trailing whitespace/newline is never added automatically (so nothing is executed in a terminal by accident).

**Language:** Auto by default (same router as meetings, but no meeting prior — each utterance stands alone, short utterances default to the last used language). Quick toggle in the pill via Tab: Auto → SV → EN.

**Dictation during an active meeting recording:** see §15 — proposed default: dictated audio is **excluded** from the meeting transcript's "Me" channel and the Me channel shows a "(dictating…)" marker.

### 8.5 Onboarding (first run)
1. Welcome + what the app does + "everything stays on this computer".
2. Choose **transcripts folder**.
3. **Download models** (shows sizes; or "Use local folder").
4. **Permissions** with guided steps and live status checks:
   - macOS: Microphone · System audio recording (Screen & System Audio Recording / audio capture) · Accessibility (for paste).
   - Linux: Microphone (PipeWire, usually no prompt) · Global shortcuts portal · Remote Desktop/input portal for paste (one-time KDE consent, persisted).
5. **Shortcuts** (dictation, meeting) with conflict check.
6. Preferences: launch at login (on), auto-detect meetings (off), dictation paste mode (preview), headphones tip.
7. Test: "Say something" → shows transcript in the pill.

### 8.6 Settings (single compact window, tabs)
General (launch at login, transcripts folder, speaker labels, language default) · Shortcuts · Dictation (mode, paste behavior, clipboard restore, terminal class list) · Meetings (auto-detect, app list, window behavior) · Vocabulary · Models · Updates · About (version, licenses, attributions).

---

## 9. Features in detail

### 9.1 Meeting transcription — see §6, §7, §8.3.

### 9.2 Dictation — see §8.4.

### 9.3 Custom vocabulary
- Two lists, editable in Settings, stored as TOML in the config dir, importable/exportable:
  1. **Terms** — preferred spellings (e.g. `HashiCorp`, `Terraform`, `Vault`, `Kubernetes`, customer names). Used for (a) **phrase boosting** in the decoder (Pianissimo; ADR 0001 as amended) and (b) case-insensitive normalization of exact matches.
  2. **Replacements** — `from → to` rules for systematic misrecognitions (e.g. `terra form → Terraform`, `hashi corp → HashiCorp`). Whole-word, case-insensitive match; preserves sentence-initial capitalization. When `to` only joins the words of `from` (or the rule is marked), `to` is boosted too; `from` never is.
- Phrase boosting is on by default and can be turned off; its strength is fixed (not a setting). It needs Pianissimo's `tokenizer.model`, an optional model file.
- Applied to final text for both meetings and dictation (and to partials, cheaply).

### 9.4 Meeting app detection (opt-in)
- macOS: Core Audio process objects (`kAudioHardwarePropertyProcessObjectList`, `kAudioProcessPropertyIsRunningInput`) → bundle IDs. Known list: Microsoft Teams (`com.microsoft.teams2`), Zoom (`us.zoom.xos`), browsers (Meet → "browser is using the microphone" = possible meeting).
- Linux: PipeWire graph — input streams (`Stream/Input/Audio`) with `application.name` / `application.process.binary` matching the list.
- List is user-editable.

### 9.5 Menu bar / tray — §8.2.

### 9.6 Autostart
- macOS: `SMAppService` (login item) via Tauri autostart plugin.
- Linux: `~/.config/autostart/tyst.desktop` pointing at the AppImage's current path (update path if the AppImage is moved/replaced).

### 9.7 Update notification
- On launch and every 24 h: query the latest GitHub Release of the repo; compare semver.
- If newer: tray badge + Settings › Updates shows version, release notes, and a "Download" button (opens the release page). **No auto-install in v1.**
- While the repo is **private**, the check requires a token: support an optional GitHub token in Settings › Updates (stored in macOS Keychain / KDE Wallet via the `keyring` crate); without a token the check is disabled and says so. Once public, no token needed.
- Can be turned off.

---

## 10. Platform implementation notes

### 10.1 macOS
- **System audio:** Core Audio **process taps** (`CATapDescription` + aggregate device), macOS 14.4+. Global tap of all output by default; optional per-process filter for meeting apps. Requires `NSAudioCaptureUsageDescription`. If Rust bindings (`objc2-core-audio`) are painful, implement a small Swift shim behind the `AudioCapture` trait. ScreenCaptureKit audio is an acceptable fallback.
- **Mic:** CoreAudio / `cpal`. Requires `NSMicrophoneUsageDescription`.
- **Non-activating floating windows:** `NSPanel` with `.nonactivatingPanel`, level `.floating` (or `.statusBar` for the pill), `canJoinAllSpaces`, `fullScreenAuxiliary` → use the `tauri-nspanel` plugin.
- **Global shortcuts:** Tauri `global-shortcut` plugin. Cmd+Å: register by key code for the Å key on the Swedish layout and verify. Known overlap: apps that read physical key position treat it as Cmd+[ (browser Back, editor outdent) — a global shortcut overrides those while Tyst runs. Ship with conflict warning + easy change.
- **Paste:** `CGEventPost` Cmd+V (key code for V); requires Accessibility permission.
- **Signing during development:** TCC permissions are tied to the code signature. Ad-hoc signed rebuilds lose granted permissions each build. Use a **stable self-signed code-signing certificate** (created once in Keychain Access) for local builds so permissions persist. Apple Developer ID signing + notarization: future.
- **Work-Mac caveat:** the Mac may be MDM-managed. If unsigned apps or the required permissions are blocked, fall back to running the locally built app and document what was blocked.

### 10.2 Arch Linux — KDE Plasma 6, Wayland
- **Audio:** PipeWire via `pipewire-rs`. Others = monitor of the default sink (follows default sink changes). Me = default source.
- **Global shortcuts:** Tauri's global-shortcut plugin does **not** work on Wayland → use the **XDG Desktop Portal GlobalShortcuts** interface (`ashpd` crate); KDE implements it and lets the user bind/confirm in System Settings.
- **Paste / key injection:** **XDG Desktop Portal RemoteDesktop** (keyboard) via `ashpd` (one-time KDE consent; request persistence). Fallback: `ydotool` (requires `ydotoold` + uinput permissions) — document as manual fallback. Note: avoid typing text character-by-character (unreliable for å/ä/ö); always paste via clipboard.
- **Clipboard:** Wayland data-control (`wl-clipboard-rs` or `arboard` with the Wayland data-control feature) so clipboard can be set/restored without focus.
- **Active-window detection (for terminal paste mode):** KWin D-Bus / small KWin script reporting the active window's resource class.
- **Always-on-top / no-focus / positioning:** Wayland does not let clients position themselves or force keep-above. Options, evaluate in Phase 4:
  1. **KWin window rules** installed by the app (`kwriteconfig6` into `kwinrulesrc` + `qdbus org.kde.KWin /KWin reconfigure`): keep above, no focus, skip taskbar/pager, remember position, pill placement at bottom-center.
  2. **wlr-layer-shell** (supported by KWin) for the pill/overlay — ideal semantics, but needs GTK layer-shell integration with Tauri's window; spike only.
  - Dragging the meeting window works natively; position persistence relies on the KWin rule.
- **Tray:** StatusNotifierItem via Tauri tray (libappindicator/ksni).
- **NVIDIA + WebKitGTK:** blank/corrupted webviews are a known issue → set `WEBKIT_DISABLE_DMABUF_RENDERER=1` (and if needed `__NV_DISABLE_EXPLICIT_SYNC=1`) in the AppImage launcher.
- **CUDA EP:** optional, off by default; CPU on a 7800X3D is more than sufficient.
- **Packaging:** AppImage via Tauri bundler; models live outside the AppImage (§5.2). `.desktop` file + icons.

---

## 11. Non-functional requirements

- **Privacy:** no audio or text leaves the device; no telemetry/analytics/crash reporting services. Only network: model download + update check, both visible and disableable.
- **Performance:** see §6.2 targets. Idle (not recording): ~0 % CPU, models may be unloaded after idle timeout.
- **Reliability:** meeting recordings of 3 h must work without memory growth (bounded buffers, streaming journal). Crash recovery per §6.6. Audio device changes (headset plugged/unplugged, default device switch) handled without stopping the session.
- **Security:** config in `~/Library/Application Support/Tyst/` / `~/.config/tyst/`; secrets (GitHub token) in OS keychain only.
- **Accessibility:** keyboard navigation in all UI; sufficient contrast; respects reduced motion.
- **Testability:** core is pure and tested with recorded fixtures (WAV → expected segments); `tyst-cli` exposes the pipeline headlessly.

---

## 12. Phases and acceptance criteria

### Phase 0 — Model evaluation spike (decision gate)
Goal: decide routing strategy, inference backend and LID approach with data from real meetings.
- Build an evaluation harness (Python with `onnx-asr`/sherpa-onnx is fine for speed of iteration; results must be reproducible by the Rust implementation later).
- **Test set** (owner provides, never committed to git; `eval/manifest.toml` references local paths):
  - ≥ 5 Swedish clips, ≥ 5 English clips (incl. Swedish-accented English), ≥ 5 mixed clips (language switch between speakers), ≥ 5 Swedish clips with English technical terms inside sentences. 30 s – 3 min each, hand-corrected reference transcripts.
  - Supplement with public FLEURS sv/en samples.
- Measure per strategy (§6.3: 1, 2, 3) and per engine:
  - WER per category; **term accuracy** on a list of key technical terms; language-switch handling (wrong-engine rate).
  - Effect of hotwords/contextual biasing and replacement rules.
  - Real-time factor and peak RSS on M1 Max (CPU; CoreML EP if available) and on the Linux box (CPU; CUDA optional).
  - Verify Pianissimo ONNX loads in sherpa-onnx; if not, document the `ort` fallback.
- **Deliverable:** `docs/phase0-report.md` + ADRs: inference backend, routing strategy, LID model, segmentation parameters.
- **Acceptance:** owner reviews report and confirms choices.

### Phase 1 — Core + CLI
- Workspace scaffolding, `tyst-core`, `tyst-cli`, CI (fmt, clippy, tests) on macOS + Linux.
- Pipeline: file input and live mic input → VAD → segments (partial/final events) → router → vocabulary → Markdown writer + journal.
- `tyst-cli transcribe <file>`, `tyst-cli live --mic`, `tyst-cli bench <manifest>`.
- **Acceptance:** CLI transcribes the Phase 0 test set with WER within 1 pp of the Phase 0 harness; live mic shows partials with ≤ 1.5 s latency on M1 Max; unit tests for segmenter, router hysteresis, vocabulary rules, Markdown writer, journal recovery.

### Phase 2 — macOS meeting MVP
- Tauri app, tray icon, onboarding, settings (General, Meetings, Models, Vocabulary).
- Mic + system audio (process tap) capture; Me/Others pipelines; floating non-activating meeting window with live transcript, scrollback, drag + remembered position, pause/stop, name-on-stop flow, toast.
- **Acceptance:** a real 30-min Teams call and a Google Meet call transcribed live without the window ever stealing focus; Markdown saved correctly; app survives headset unplug mid-call; crash recovery works (kill -9 mid-meeting → recovery prompt on relaunch).

### Phase 3 — macOS dictation
- Global shortcut (Cmd+Å default, configurable, conflict warning), toggle + push-to-talk hybrid, pill UI, preview mode + direct mode, paste into remembered app, clipboard restore, Esc cancel, Tab language toggle.
- **Acceptance:** dictation works in Slack, browser text fields, Cursor/VS Code, Notes and Ghostty; Swedish characters correct; clipboard restored; end-of-speech → text visible ≤ 1 s.

### Phase 4 — Arch Linux / KDE Wayland port
- PipeWire capture, portal global shortcuts, portal key injection, Wayland clipboard, terminal detection, KWin rules for window behavior, tray, autostart, AppImage packaging, NVIDIA webview workaround.
- **Acceptance:** Phases 2–3 acceptance scenarios pass on Arch/KDE Wayland (Teams/Meet/Zoom in browser or native clients); AppImage runs from `~/.local/share/AppImage/`.

### Phase 5 — Polish & public readiness
- Opt-in meeting auto-detection; update notifications (§9.7); echo cancellation (§6.4); "only meeting apps" audio filter (macOS); README with screenshots, THIRD_PARTY_NOTICES, About view attributions; release workflow (GitHub Actions building macOS `.app`/`.dmg` and Linux AppImage on tag).
- **Acceptance:** owner has used the app daily for ≥ 2 weeks; open bugs triaged; attribution and licenses complete for going public.

---

## 13. Licensing and attribution

Required in `THIRD_PARTY_NOTICES.md`, the About view and the README:
- **Klang Pianissimo** — © Klang AI AB, CC BY 4.0 — `https://huggingface.co/KlangAI/pianissimo-sv`. Cite as given in the model card.
- **NVIDIA Parakeet TDT 0.6B v3** — © NVIDIA, CC BY 4.0.
- **Silero VAD** — MIT. **sherpa-onnx** — Apache-2.0. **ONNX Runtime** — MIT. **Tauri** — MIT/Apache-2.0. Plus all Rust/npm dependencies (generate with `cargo-about` / `license-checker`).
- Any LID model used (e.g. Whisper — MIT).
- Mention prior art / inspiration where appropriate (other open Pianissimo projects) — no code copied without checking licenses.
- Repo is **private** initially; goal is to make it public once stable.

---

## 14. Future ideas (out of scope for v1)

- Summaries / action items via an LLM (local, or Claude etc. depending on company policy).
- Optional per-turn timestamps in Markdown.
- Diarization within "Others" (multiple remote speakers).
- Optional audio retention for re-transcription.
- Search across saved transcripts; Obsidian-friendly frontmatter/tags.
- Apple Developer ID signing + notarization; in-app auto-update (Sparkle on macOS, AppImageUpdate/zsync on Linux).
- CoreML / Apple Neural Engine backend (existing CoreML conversions of Pianissimo).
- More languages as Klang releases new models.
- Windows support.
- iOS app (SnickSnack covers mobile for now).

---

## 15. Open questions (ask the owner before deciding)

1. **App name** (working name "Tyst").
2. **App license** for the public release (suggested: MIT or Apache-2.0).
3. **Meeting shortcut** default — proposed Cmd+Shift+Å / Ctrl+Shift+Å.
4. **Dictation during a meeting** — proposed: exclude dictated audio from the meeting transcript's Me channel. Confirm.
5. **Dictation trigger** — proposed hybrid (tap = toggle, hold = push-to-talk). Confirm or pick one.
6. **Speaker labels** — "Me/Others" vs the owner's name.
7. Whether the "only meeting apps" system-audio filter should be the default on macOS (avoids transcribing notification sounds/music, but misses audio from unknown apps).
