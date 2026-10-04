# Phase 3 report: dictation

- **Status:** Accepted on Arch Linux / KDE Plasma (Wayland) by the owner on 2026-10-04 (section 5).
  macOS code compiles in CI but has not run on a Mac (testing there is postponed).
- **Code:** `crates/tyst-runtime/src/dictation.rs`, `crates/tyst-platform/src/{clipboard,portal,kwin,macos_input}.rs`
  (feature `desktop`), `app/src-tauri/src/{dictation,desktop,shortcuts}.rs`, `app/ui/src/pill/`,
  `crates/tyst-cli/src/dictate.rs`.

## 1. What exists

| SPEC item | Where |
|---|---|
| Global shortcut, default Ctrl+Å (Linux) / Cmd+Å (macOS), configurable, conflict warning | `shortcuts.rs`: XDG GlobalShortcuts portal on Linux (KDE confirms the keys and lets the user rebind them), Tauri global-shortcut plugin on macOS |
| Toggle + push-to-talk hybrid (tap toggles, hold > 400 ms talks until release); toggle-only and hold-only | `dictation.rs` `Gesture` (portal reports press and release) |
| Pill at the bottom centre: waveform, live text, language badge | `app/ui/src/pill/Pill.svelte`; placed by KWin script on KDE, by the app elsewhere |
| Preview mode (editable text, Enter paste, Ctrl+C copy, Esc discard; the shortcut again records more and adds it, tap or hold) and direct mode ("Pasted" flash with Copy) | `Pill.svelte`, `dictation.rs` |
| Paste into the window that was active at start | KWin script remembers and re-activates the window; RemoteDesktop portal sends Ctrl+V (Ctrl+Shift+V in terminals), `ydotool` fallback; macOS: NSWorkspace + CGEvent Cmd+V |
| Clipboard restored after paste | `desktop.rs`, 500 ms after the keystroke; the transient text is hidden from clipboard history |
| Esc cancels, Tab cycles Auto → SV → EN | while listening the switch applies to the next segment; in preview the kept audio is decoded again |
| Dictation during a meeting (SPEC 15 q4, confirmed) | the Me channel skips dictated audio and the transcript gets a `*(dictating…)*` marker |
| Meeting shortcut Ctrl+Shift+Å (SPEC 15 q3, confirmed) | same portal session |

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
Owner run on Arch / KDE Plasma (Wayland), 2026-10-04: the five apps, Swedish characters, clipboard
restore, Esc/Tab, direct mode and dictation during a meeting pass. Hold vs tap failed on one point:
the shortcut in the preview pasted instead of recording more; it now records more, and the re-test passed.
In the app, listening began 38–48 ms after the shortcut and the text was ready 1–101 ms after stop.
`tyst-cli dictate` on the owner's 10 Swedish clips: stop → text p50 0.13 s, p95/max 0.25 s.

## 6. Decisions

The owner confirmed the SPEC 15 proposals on 2026-10-03, as built:
- q3: meeting shortcut Ctrl+Shift+Å (Cmd+Shift+Å on macOS).
- q4: dictated audio is left out of the meeting's Me channel, with a *(dictating…)* marker.
- q5: hybrid trigger (tap toggles, hold is push-to-talk); toggle-only and hold-only stay as settings.
