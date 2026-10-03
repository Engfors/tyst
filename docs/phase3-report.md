# Phase 3 report: dictation

- **Status:** Built; awaiting owner acceptance on Arch Linux / KDE Plasma (Wayland). macOS code
  compiles in CI but has not run on a Mac (testing there is postponed).
- **Code:** `crates/tyst-runtime/src/dictation.rs`, `crates/tyst-platform/src/{clipboard,portal,kwin,macos_input}.rs`
  (feature `desktop`), `app/src-tauri/src/{dictation,desktop,shortcuts}.rs`, `app/ui/src/pill/`,
  `crates/tyst-cli/src/dictate.rs`.

## 1. What exists

| SPEC item | Where |
|---|---|
| Global shortcut, default Ctrl+Å (Linux) / Cmd+Å (macOS), configurable, conflict warning | `shortcuts.rs`: XDG GlobalShortcuts portal on Linux (KDE confirms the keys and lets the user rebind them), Tauri global-shortcut plugin on macOS |
| Toggle + push-to-talk hybrid (tap toggles, hold > 400 ms talks until release); toggle-only and hold-only | `dictation.rs` `Gesture` (portal reports press and release) |
| Pill at the bottom centre: waveform, live text, language badge | `app/ui/src/pill/Pill.svelte`; placed by KWin script on KDE, by the app elsewhere |
| Preview mode (editable text, Enter paste, Ctrl+C copy, Esc discard) and direct mode ("Pasted" flash with Copy) | `Pill.svelte`, `dictation.rs` |
| Paste into the window that was active at start | KWin script remembers and re-activates the window; RemoteDesktop portal sends Ctrl+V (Ctrl+Shift+V in terminals), `ydotool` fallback; macOS: NSWorkspace + CGEvent Cmd+V |
| Clipboard restored after paste | `desktop.rs`, 500 ms after the keystroke; the transient text is hidden from clipboard history |
| Esc cancels, Tab cycles Auto → SV → EN | while listening the switch applies to the next segment; in preview the kept audio is decoded again |
| Dictation during a meeting (SPEC 15 q4 as proposed) | the Me channel skips dictated audio and the transcript gets a `*(dictating…)*` marker |
| Meeting shortcut Ctrl+Shift+Å (SPEC 15 q3 as proposed) | same portal session |

Also: a Dictation settings tab, an onboarding step (shortcut + paste permission), a tray item,
`tyst --dictate` for binding a key by hand, and `tyst-cli dictate` (`--mic`, or `--simulate` files
played in real time with `--stop-after-speech`) for latency measurement without the UI.

## 2. How a dictation runs

Capture starts on the shortcut. Audio goes through the same VAD-segmented pipeline as meetings
(ADR 0004), with partials on for the live text. On stop, partials are switched off and only the open
tail is decoded, so the final text is ready shortly after the last word. If the VAD found no
segment but the recording has speech-level audio (≥ 0.3 s, peak ≥ 0.02), the whole recording is
decoded. Up to 300 s of 16 kHz audio is kept so Tab can decode it again in another language.

## 3. Measurements

`tyst-cli dictate --simulate <10 FLEURS sv clips> --stop-after-speech 0.2` on a 4-core container
(slower than the target machines):

| | p50 | p95 | max |
|---|---|---|---|
| stop → final text | 0.26 s | 0.63 s | 0.63 s |
| start → first live text | 1.8 s | | |

Target: end-of-speech → text ≤ 1.0 s. Met on every clip.

In-app smoke test (Xvfb, PipeWire virtual mic, no portal): listening 40 ms after the shortcut,
text ready 1–2 ms after stop (the tail was already decoded), clipboard set in ~95 ms. With no portal
or ydotool the paste falls back to clipboard-only, as designed.

## 4. Known limits

- A shortcut pressed while models are still loading (the first seconds after app start) begins
  capture only once they are loaded, so words spoken before that are lost.
- Pasting needs the RemoteDesktop portal (KDE asks once; Tyst keeps the restore token) or `ydotool`.
  Window re-activation and pill placement use KWin scripting, so on other Wayland desktops focus
  return is left to the compositor.
- macOS paste needs the Accessibility permission; onboarding and settings explain where to grant it, and macOS asks on the first paste.

## 5. Acceptance

Steps: Arch test steps shared with the owner (five apps, Swedish characters, latency, clipboard
restore, hold vs tap, Esc/Tab, direct mode, dictation during a meeting, CLI latency on the owner's clips).
Results: pending.

## 6. Open questions

SPEC 15 q3 (meeting shortcut), q4 (exclude dictated audio from Me) and q5 (hybrid trigger) are built
as proposed and await the owner's confirmation.
