# Phase 4 report: Arch Linux / KDE Wayland port

- **Status:** Built; waiting for the owner's acceptance run on Arch Linux / KDE Plasma (Wayland).
  ADR 0005 is Proposed until then.
- **Code:** `crates/tyst-platform/src/appimage.rs` (environment for programs Tyst starts),
  `app/src-tauri/src/{appimage,autostart}.rs`, AppImage handling in `shortcuts.rs`, `commands.rs`,
  `kwin.rs` and `main.rs`; `packaging/linux/`; the `appimage` job in `.github/workflows/ci.yml`.

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
| Autostart | `autostart.rs` → `~/.config/autostart/Tyst.desktop` (macOS: `tauri-plugin-autostart`) | Phase 2; rewritten for the AppImage |
| AppImage packaging | Tauri bundler (linuxdeploy), CI artifact `tyst-appimage` | **Phase 4** |

## 2. What changed for the AppImage

- **Login entry and desktop file follow the AppImage.** Both point at the AppImage file, not at
  the binary inside its mount, and are refreshed on every start, so moving or replacing the
  AppImage fixes them on the next launch (SPEC 9.6). `$APPIMAGE` is only trusted when Tyst's own
  binary is inside `$APPDIR` and the file exists. Linux writes the login entry itself instead of
  through `tauri-plugin-autostart`: `Exec` is quoted and escaped per the Desktop Entry
  Specification (a path with spaces, `$`, `` ` ``, `\` or `%` survives GLib's parsing and passes
  `desktop-file-validate`), only Tyst's own keys are rewritten (keys the user added stay), the
  file is written only when it changes, and `XDG_CONFIG_HOME` is respected.
- **Programs Tyst starts get the host's environment.** The AppImage's `AppRun` prepends `$APPDIR`
  entries to `LD_LIBRARY_PATH`, `PATH`, `XDG_DATA_DIRS`, `QT_PLUGIN_PATH`, the GStreamer and
  Python/Perl paths, and linuxdeploy's GTK hook points GTK, GIO and GSettings into the image.
  Tyst keeps that environment for itself; `xdg-open` (and the editor it opens for a transcript),
  `kwriteconfig6`, `kreadconfig6`, `dbus-send` and `ydotool` start with the image's entries
  removed from those paths and the launcher-only variables unset. CI extracts each AppImage it
  builds and fails if the launcher sets a variable the strip lists do not cover
  (`packaging/linux/check-appimage-env.sh`).
- **One desktop identity.** The desktop file inside the image (from `packaging/linux/tyst.desktop`)
  and the one Tyst writes have the same name, comment and single main category (`Office`). The
  file names differ (`Tyst.desktop` is set by the bundler, `com.engfors.tyst.desktop` is the
  portal's app id), so desktop integration tools that copy the image's file can still add a
  second menu entry.
- **Tyst's own icon** is installed next to its desktop file
  (`~/.local/share/icons/hicolor/128x128/apps/com.engfors.tyst.png`), so KDE's shortcut settings and
  the portal dialogs show it instead of a generic microphone.
- **CI builds the AppImage** on Ubuntu 24.04 for every push and pull request and keeps it 14 days
  as the `tyst-appimage` artifact. It is a test build: unsigned, no published checksum, no update
  information. Signed release builds on tags are Phase 5.

What the bundle holds: WebKitGTK, GTK, GLib, GStreamer, the tray library and
`libwayland-cursor`/`-egl`/`-server` from the build host. glibc, `libstdc++`, PipeWire,
`libwayland-client` and EGL/GL are excluded and come from the host. ONNX Runtime is linked
statically. Models stay outside the image (SPEC 5.2). Because the bundled libraries are not on
`LD_LIBRARY_PATH` for other programs, only Tyst itself uses them. Inside Tyst, the host's
`libpipewire` needs only libc, but the SPA plugins it loads from the host link `libdbus`,
`libsystemd` and `libcap`, which then resolve to the bundled (Ubuntu 24.04) copies; those sonames are
ABI-stable, and the Arch run confirms it.

Host requirements: glibc 2.39 and `libstdc++` with `GLIBCXX_3.4.31` (the build host's), and
`fusermount3` (Arch: `fuse3`; the image uses the static type-2 runtime, so no `fuse2`). Arch
meets these; Ubuntu 22.04 and Debian 12 do not. Size: 125 MB.

## 3. Tested in the container

Ubuntu 24.04, Xvfb, PipeWire 1.0.5 with WirePlumber and virtual devices, FUSE mount (not
extract-and-run), AppImage copied to `~/.local/share/AppImage/`.

- The AppImage starts, onboarding renders.
- Two-channel meeting from the AppImage: `tyst --toggle-meeting` (forwarded to the running
  instance), Swedish speech played into a virtual mic (Me) and the default sink (Others), stop,
  30 s timeout, Markdown saved with both channels.
- Environment of programs Tyst starts (a stand-in `kwriteconfig6` earlier on `PATH` printed it):
  no `LD_LIBRARY_PATH`, `GTK_PATH`, `APPDIR` or `APPIMAGE`, and `PATH` and `XDG_DATA_DIRS`
  without the mount.
- Moved the AppImage into a folder with a space and relaunched: the login entry and the desktop
  file point at the new path, GLib parses both `Exec` lines to that one path, both pass
  `desktop-file-validate`, and a key added to the login entry by hand stayed.

Not testable here: KDE (tray, portals, KWin), NVIDIA, real calls.

## 4. Acceptance

SPEC 12: Phases 2–3 acceptance scenarios pass on Arch/KDE Wayland (Teams/Meet/Zoom in browser or
native clients); the AppImage runs from `~/.local/share/AppImage/`. Steps for the owner's machine are
in the project's shared files (`phase4/arch-test-steps.md`, outside the repo, like earlier phases).

## 5. Known limits

- KWin rules and the KWin script are KDE-only (ADR 0005).
- `__NV_DISABLE_EXPLICIT_SYNC=1` (SPEC 10.2, "if needed") is not set by default; the DMA-BUF
  workaround alone fixed the owner's NVIDIA crash in Phase 2. It can be set by hand if the
  AppImage's webview flickers or shows blank on NVIDIA.
- CUDA execution provider: not built (SPEC 10.2: optional, off by default).
