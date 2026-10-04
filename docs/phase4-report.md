# Phase 4 report: Arch Linux / KDE Wayland port

- **Status:** Built; waiting for the owner's acceptance run on Arch Linux / KDE Plasma (Wayland).
- **Code:** `app/src-tauri/src/appimage.rs`, AppImage handling in `shortcuts.rs`, `commands.rs`,
  `kwin.rs` and `main.rs`; the `appimage` job in `.github/workflows/ci.yml`.

## 1. Scope

The owner tests on Arch, so most of SPEC 12's Phase 4 list was built and accepted there in Phases 2
and 3. Phase 4 adds the AppImage and what changes when Tyst runs from one.

| SPEC 12 Phase 4 item | Where | Since |
|---|---|---|
| PipeWire capture (default source, default sink monitor, follows device changes) | `tyst_platform::pipewire` | Phase 2 |
| Portal global shortcuts | `shortcuts.rs` (XDG GlobalShortcuts) | Phase 3 |
| Portal key injection (`ydotool` fallback) | `tyst_platform::portal` (RemoteDesktop) | Phase 3 |
| Wayland clipboard | `tyst_platform::clipboard` (data-control) | Phase 3 |
| Terminal detection (Ctrl+Shift+V) | KWin script reports the active window's class | Phase 3 |
| KWin rules for window behavior | `kwin.rs`; decision in [ADR 0005](decisions/0005-linux-window-behavior.md) | Phase 2–3 |
| Tray (StatusNotifierItem) | `tray.rs` | Phase 2 |
| NVIDIA webview workaround | `main.rs` sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` | Phase 2 |
| Autostart | `tauri-plugin-autostart` → `~/.config/autostart/Tyst.desktop` | Phase 2; now follows the AppImage |
| AppImage packaging | Tauri bundler (linuxdeploy), CI artifact `tyst-appimage` | **Phase 4** |

## 2. What changed for the AppImage

- **Login entry and desktop file follow the AppImage.** Both point at the AppImage file
  (`$APPIMAGE`), not at the binary inside its mount, and are rewritten on every start, so moving or
  replacing the AppImage fixes them on the next launch (SPEC 9.6). Before, the login entry was
  only written when the setting changed.
- **Programs Tyst starts get the host's environment.** The AppImage launcher points GTK, GIO and
  GSettings into the mounted image (`GTK_PATH`, `GIO_MODULE_DIR`, `XDG_DATA_DIRS`, …). Without a
  fix, the editor opened from the "Saved · Open" toast or "Open transcripts folder" would inherit
  that and load Tyst's bundled GTK modules. `xdg-open`, `kwriteconfig6` and `kreadconfig6` now
  start without those variables.
- **Tyst's own icon** is installed next to the desktop file
  (`~/.local/share/icons/hicolor/128x128/apps/com.engfors.tyst.png`), so KDE's shortcut settings and
  the portal dialogs show it instead of a generic microphone.
- **CI builds the AppImage** on Ubuntu 24.04 for every push and pull request and keeps it 14 days
  as the `tyst-appimage` artifact. The release workflow on tags is Phase 5.

What the bundle holds: WebKitGTK, GTK and the tray library from the build host. PipeWire, Wayland,
EGL/GL and glibc are excluded and come from the host, so capture talks to the host's PipeWire
version. ONNX Runtime is linked statically. Models stay outside the image (SPEC 5.2). The AppImage
uses the static type-2 runtime, so it needs `fusermount3` (Arch: `fuse3`), not `fuse2`.
Size: 125 MB.

## 3. Tested in the container

Ubuntu 24.04, Xvfb, PipeWire 1.0.5 with WirePlumber and virtual devices, FUSE mount (not
extract-and-run), AppImage copied to `~/.local/share/AppImage/`.

- The AppImage starts, onboarding renders.
- Two-channel meeting from the AppImage: `tyst --toggle-meeting` (forwarded to the running
  instance), Swedish speech played into a virtual mic (Me) and the default sink (Others), stop,
  30 s timeout, Markdown saved with both channels.
- Environment of programs Tyst starts (checked with stand-ins for `kwriteconfig6`): no `GTK_PATH`,
  `APPDIR` or `APPIMAGE`, and `XDG_DATA_DIRS` without the mount.
- Moved the AppImage and relaunched: the login entry and the desktop file point at the new path.

Not testable here: KDE (tray, portals, KWin), NVIDIA, real calls.

## 4. Acceptance

SPEC 12: Phases 2–3 acceptance scenarios pass on Arch/KDE Wayland (Teams/Meet/Zoom in browser or
native clients); the AppImage runs from `~/.local/share/AppImage/`. Steps for the owner's machine are
in the project files (`phase4/arch-test-steps.md`).

## 5. Known limits

- KWin rules and the KWin script are KDE-only (ADR 0005).
- `__NV_DISABLE_EXPLICIT_SYNC=1` (SPEC 10.2, "if needed") is not set by default; the DMA-BUF
  workaround alone fixed the owner's NVIDIA crash in Phase 2. It can be set by hand if the
  AppImage's webview flickers or shows blank on NVIDIA.
- CUDA execution provider: not built (SPEC 10.2: optional, off by default).
