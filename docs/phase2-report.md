# Phase 2 report: meeting app

- **Status:** In progress. Built and tested end to end on Linux (container); waiting for the owner's
  acceptance run on Arch Linux / KDE (section 5).
- **Code:** `app/` (Tauri v2 + Svelte 5), `crates/tyst-runtime`, capture in `crates/tyst-platform`.

## 1. Scope decision

SPEC 12 makes Phase 2 a macOS meeting MVP and Phase 4 the Linux port. The owner tests on Arch Linux
(KDE Plasma, Wayland), so Phase 2 is built and accepted on Linux first: PipeWire capture and a KWin
window rule are pulled forward from Phase 4. The macOS system-audio tap is written and compiled in CI
but has not run on a Mac, and the macOS non-activating panel (`NSPanel`) is not done yet (section 6).

## 2. What exists

| SPEC item | Where |
|---|---|
| Tauri app, tray icon (no Dock icon / taskbar entry), icon states | `app/src-tauri/src/tray.rs`, `main.rs` |
| Onboarding: folder, model download, audio check, preferences, test | `app/ui/src/onboarding/` |
| Settings: General, Meetings, Models, Vocabulary, About | `app/ui/src/settings/`, `app/ui/src/lib/` |
| Mic + system audio capture, one pipeline per channel | `tyst_platform::pipewire` (Linux), `tyst_platform::macos_tap` (macOS), `tyst_runtime::meeting` |
| Floating meeting window: live transcript, partials, level meters, language badge, scrollback with "↓ Latest", compact mode, pause/stop | `app/ui/src/meeting/Meeting.svelte`, `app/src-tauri/src/windows.rs` |
| Never steal focus | window created unfocused and non-focusable; KWin rule on KDE (`kwin.rs`) |
| Drag + remembered position | drag region in the header; geometry saved per display; on Wayland the KWin rule remembers it |
| Name-on-stop, 30 s timeout, Esc, toast "Saved · Open · Show in folder" | `state.rs` (`stop_meeting`, `save_meeting`), `Meeting.svelte` |
| Pause marker in the transcript | `MarkerKind::Paused`, rendered as `*Paused*` |
| Survive headset unplug | PipeWire streams follow the default device; the cpal mic and the macOS tap rebuild on a default-device change |
| Crash recovery prompt | `main.rs` `offer_recovery` (journal → Markdown on relaunch) |

Also new: `tyst-cli meeting` runs the same two-channel session headlessly (`--mic --system`, or
`--me/--others <file>` played in real time), and `tyst --toggle-meeting` / `--pause` act on the
running app, so a KDE custom shortcut can start and stop meetings until global shortcuts arrive.

### Timeline

Each channel has a capture thread and a pipeline worker. Segment times come from capture
timestamps: when a channel has a gap (pause, a system-audio stream that only runs while something
plays, a device switch) its worker ends the open segment and moves that channel's clock forward, so
Me and Others stay on one wall-clock timeline.

## 3. First words were being cut (fixed)

Live tests showed the first word of a sentence missing ("Låter bra" → "Bra"). The cause is in the
Phase 1 segmenter, not capture: sherpa's VAD state machine restarts its 250 ms speech count on every
window that dips below the threshold, so a soft onset triggers several hundred ms late, and the fixed
0.31 s pre-roll does not reach back far enough. Segments now start where the run of windows above
the hysteresis floor (`threshold − 0.15`) began, at most 0.75 s before the fixed pre-roll.

`tyst-cli bench` on the 55 FLEURS clips (container):

| category | before | after |
|---|---|---|
| all | 19.9 | **18.7** |
| fleurs-sv | 10.4 | **9.0** |
| fleurs-en | 22.9 | **21.5** |
| fleurs-mixed | 25.8 | **25.2** |

The owner clips should be re-run on the owner's machine (section 5).

## 4. Tested in the container

Ubuntu 24.04, PipeWire 1.0.5 with WirePlumber and virtual devices, Xvfb.

- Two-channel meeting from the virtual mic and the default sink's monitor: both channels transcribed,
  merged by time, Markdown saved.
- Default source switched mid-meeting (old one removed): recording continued on the new mic.
- The app on a virtual display: live text, pause (marker in the file), stop, typed title, Enter,
  file saved, toast.
- `kill -9` during a meeting, relaunch: recovery prompt, Markdown written, journal removed.

Not testable here: focus behavior under KWin, the tray on a real panel, real Teams/Meet audio.

## 5. Acceptance on the owner's machine

SPEC 12: a real 30-min Teams call and a Google Meet call transcribed live without the window ever
stealing focus; Markdown saved correctly; the app survives a headset unplug mid-call; crash recovery
works (kill -9 mid-meeting → recovery prompt on relaunch). Step-by-step instructions for Arch are in
the project files (`phase2/arch-test-steps.md`). The app is built with
`cd app/ui && npm ci && npm run tauri build -- --no-bundle`.

## 6. Open items

- macOS: run the process tap on a Mac; make the meeting window an `NSPanel` with
  `nonactivatingPanel` so clicking it does not activate Tyst (today it is non-focusable only).
- Meeting shortcut default (SPEC 15 question 3) and global shortcuts arrive with Phase 3; until then
  `tyst --toggle-meeting` can be bound to a key in KDE.
- App name and bundle identifier (`com.engfors.tyst`) follow the working name (SPEC 15 question 1).
- Unloading models after an idle timeout (SPEC 5.2) is not implemented; models stay loaded.
