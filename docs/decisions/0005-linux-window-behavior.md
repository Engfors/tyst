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
- Desktops other than KDE and Hyprland get ordinary windows: the meeting window may take focus
  when shown and its position is up to the compositor; dictation still works, but focus return
  before the paste is left to the compositor. Hyprland is the same option 1, below, not a
  layer-shell spike.
- The rules live in the user's `kwinrulesrc` under fixed group names, so reinstalling replaces
  them; they stay behind if Tyst is removed (Settings › Meetings can reinstall them, and KDE's
  Window Rules settings can delete them).

### Update (2026-10-31) — Hyprland, same choice

Omarchy (Hyprland) gets the same option 1, not layer-shell: Tyst writes the rules and does not
edit the user's Hyprland config. Hyprland IPC does what rules cannot express (pill placement,
focus return, meeting-window position). `no_initial_focus` is used instead of `no_focus`, so
showing a window does not take the keyboard and the name field can still be typed into. KDE
behavior above is unchanged.

### Update (2026-11-06) — Lua configs do not `source` the classic file

The 2026-10-31 note described only the classic file. Current Omarchy is Hyprland 0.56 with a
Lua-only config, where `source =` is invalid. Tyst writes both files:

- `hyprland.lua` calls upstream `hl.window_rule` (float, pin, `no_initial_focus`, no border).
  It does not call Omarchy's `o.window`, so the file loads without `o`. The user adds
  `dofile(<that file>)` in `~/.config/hypr/hyprland.lua` after `require("default.hypr.omarchy")`.
- `hyprland.conf` is the Hyprland 0.53 `windowrule` form for a classic config. The user adds
  `source = <that file>`.

Shortcut fallback is separate from window rules. When the GlobalShortcuts portal does not bind a
key, Omarchy adds `o.bind(...)` in `~/.config/hypr/bindings.lua`. A classic config uses `bind =`
in `hyprland.conf`. Paste on Hyprland is `send_key_state` (key down, then key up), not `wtype`.

## Evidence

Owner acceptance on Arch / KDE Plasma 6 (Wayland): Phase 2 runs A, B and E (the window never took
focus during two 30-minute calls; its position survived a restart) and Phase 3 runs A–G (pill
placement and paste into the remembered window in five apps).
