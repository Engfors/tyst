# 0005 · Linux window behavior: KWin rules, not layer-shell

- **Status:** Accepted (owner, 2026-10-04)
- **Date:** 2026-10-04

## Context

SPEC 10.2 asks Phase 4 to choose how the meeting window and the dictation pill stay on top, never
take focus and get placed on KDE Plasma under Wayland, where clients cannot do this themselves.
Option 1 is KWin window rules written by the app; option 2 is wlr-layer-shell (supported by KWin)
through GTK layer-shell, flagged as a spike.

## Decision

KWin window rules (option 1), plus a KWin script for what rules cannot express. The rules
(`app/src-tauri/src/kwin.rs`) force keep-above, skip taskbar/pager/switcher, extreme focus-stealing
prevention and no border for both windows, and remember the meeting window's position and size. The
script (`tyst_platform::kwin`) places the pill at the bottom centre of the active screen and
re-activates the window that was active when dictation started.

No layer-shell spike.

## Consequences

- It works with Tauri's normal GTK windows, so the meeting window keeps native dragging and
  resizing, and clicking into the name-on-stop field still focuses it (layer-shell surfaces need
  keyboard interactivity set up front and cannot be moved by dragging).
- It is KDE-only. Other Wayland desktops get ordinary windows: the meeting window may take focus
  when shown and its position is up to the compositor; dictation still works, but focus return
  before the paste is left to the compositor.
- The rules live in the user's `kwinrulesrc` under fixed group names, so reinstalling replaces
  them; they stay behind if Tyst is removed (Settings › Meetings can reinstall them, and KDE's
  Window Rules settings can delete them).

## Evidence

Owner acceptance on Arch / KDE Plasma 6 (Wayland): Phase 2 runs A, B and E (the window never took
focus during two 30-minute calls; its position survived a restart) and Phase 3 runs A–G (pill
placement and paste into the remembered window in five apps).
