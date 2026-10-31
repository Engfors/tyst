//! Hyprland IPC (Omarchy and other Hyprland sessions): the active window (to paste back into
//! it and to spot terminals), focusing it again, placing the dictation pill at the bottom centre
//! of the focused monitor, and reading and restoring the meeting window's position.
//!
//! Wayland clients cannot do this themselves. The socket is the one `hyprctl` uses
//! (`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`). Commands are the same
//! strings `hyprctl` writes; replies are read back and the connection closes. Our own windows
//! are recognised by this process's pid together with the title. A title on its own is not
//! enough: another client can set the same title. The Wayland class is not used: GTK sets it
//! from the program name unless `enableGTKAppId` is on. Titles are matched locally and then
//! dropped: they are never logged and never sent back to Hyprland.
//!
//! Coordinates are Hyprland's global layout space, the same space as a client's `at` field and
//! as `movewindowpixel exact`. Monitor `width` / `height` from `hyprctl monitors` are the mode
//! in physical pixels; `x`, `y`, window `at` / `size` and reserved zones are logical pixels
//! (physical size divided by `scale`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::DesktopError;

const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPLY: usize = 2 * 1024 * 1024;

/// A window Hyprland can focus, addressed by the hex id `hyprctl` prints (`0x…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRef {
    /// Hyprland address, `0x` plus hex digits. Stable while the window exists.
    pub id: String,
    /// Wayland app id, or the X11 class. The same string KWin calls a resource class.
    pub class: String,
}

/// A mapped window's address, class, position and size. The title is not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedWindow {
    pub id: String,
    pub class: String,
    pub at: (i32, i32),
    pub size: (i32, i32),
    floating: bool,
}

/// Layer-shell exclusive zone, in logical pixels. Hyprland's `reserved` array is
/// `[left, top, right, bottom]` (`m_reservedArea` left, top, right, bottom in `hyprctl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reserved {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// One monitor, in the layout `hyprctl monitors -j` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    /// Mode width in physical pixels.
    pub width: i32,
    /// Mode height in physical pixels.
    pub height: i32,
    pub scale: f64,
    pub focused: bool,
    pub reserved: Reserved,
}

impl Monitor {
    /// Size of the monitor in the global layout (logical pixels).
    pub fn logical_size(&self) -> (i32, i32) {
        (logical_len(self.width, self.scale), logical_len(self.height, self.scale))
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        let (w, h) = self.logical_size();
        x >= self.x && y >= self.y && x < self.x.saturating_add(w) && y < self.y.saturating_add(h)
    }
}

/// True when this session is Hyprland and not KDE (a `KDE` desktop wins if both are set).
/// `xdg` is `XDG_CURRENT_DESKTOP`; an empty value counts as unset, and then a non-empty
/// `HYPRLAND_INSTANCE_SIGNATURE` is enough.
pub fn detect_hyprland(kde: bool, xdg: Option<&str>, signature: Option<&str>) -> bool {
    if kde {
        return false;
    }
    match xdg.map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => d.split(':').any(|p| p.eq_ignore_ascii_case("Hyprland")),
        None => signature.is_some_and(|s| !s.trim().is_empty()),
    }
}

/// True on a Hyprland session (Omarchy sets `XDG_CURRENT_DESKTOP=Hyprland`).
pub fn is_hyprland() -> bool {
    let xdg = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok();
    detect_hyprland(crate::kwin::is_kde(), xdg.as_deref(), sig.as_deref())
}

/// `0x` plus hex, not the null address `0x0` Hyprland uses for "no window".
pub fn is_window_address(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("0x") else { return false };
    !hex.is_empty() && hex.len() <= 16 && hex.chars().all(|c| c.is_ascii_hexdigit()) && hex.chars().any(|c| c != '0')
}

/// Which `dispatch` grammar this compositor accepts. Hyprland 0.56 with a Lua config rejects
/// the classic `focuswindow` / `movewindowpixel` lines (`hl.dispatch(...): ')' expected near ...`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchKind {
    Classic,
    Lua,
}

pub struct Hyprland {
    socket: PathBuf,
    /// This process. Hyprland reports it from the client credentials, which another window cannot set.
    pid: i64,
    /// How long one socket round-trip may take.
    timeout: Duration,
    /// Set only after a definitive probe. An IO error, timeout, or unexpected reply leaves this empty.
    dispatch: std::sync::Mutex<Option<DispatchKind>>,
}

impl Hyprland {
    /// Connects if the instance socket exists.
    pub fn connect() -> Result<Self, DesktopError> {
        let socket = socket_path()?;
        if !socket.exists() {
            return Err(DesktopError::Hyprland(format!("no socket at {}", socket.display())));
        }
        Ok(Self { socket, pid: i64::from(std::process::id()), timeout: TIMEOUT, dispatch: std::sync::Mutex::new(None) })
    }

    /// The focused window, if there is one. This is the paste target, so it is not limited to our pid.
    pub async fn active_window(&self) -> Result<Option<WindowRef>, DesktopError> {
        parse_active_window(&self.command("j/activewindow").await?)
    }

    /// Focuses the window with this address. False if that address is not mapped.
    pub async fn activate(&self, id: &str) -> Result<bool, DesktopError> {
        if !is_window_address(id) {
            return Ok(false);
        }
        let clients = parse_clients(&self.command("j/clients").await?)?;
        if !clients.iter().any(|c| c.address == id) {
            return Ok(false);
        }
        self.focus_address(id).await?;
        Ok(true)
    }

    /// Focuses our window whose title is exactly `title`. False if it is not mapped.
    pub async fn focus_title(&self, title: &str) -> Result<bool, DesktopError> {
        let Some(window) = self.own_window(title).await? else { return Ok(false) };
        self.focus_address(&window.id).await?;
        Ok(true)
    }

    /// Moves our window titled `title` to the bottom centre of the focused monitor, `bottom`
    /// logical pixels above the work area's lower edge, and optionally gives it the keyboard.
    /// False if no such window is mapped yet (the caller retries while it maps).
    pub async fn place_bottom_center(&self, title: &str, bottom: i32, focus: bool) -> Result<bool, DesktopError> {
        let Some(window) = self.own_window(title).await? else { return Ok(false) };
        if window.size.0 <= 0 || window.size.1 <= 0 {
            return Ok(false);
        }
        let monitors = self.monitors().await?;
        let Some(monitor) = focused_monitor(&monitors) else { return Ok(false) };
        let (x, y) = pill_top_left(monitor, window.size, bottom);
        self.move_window(&window, x, y).await?;
        if focus {
            self.focus_address(&window.id).await?;
        }
        Ok(true)
    }

    /// Top-left of our window titled `title`, in global logical pixels, or `None` if it is not
    /// mapped. (A Wayland client cannot read its own position.)
    pub async fn position(&self, title: &str) -> Result<Option<(i32, i32)>, DesktopError> {
        Ok(self.own_window(title).await?.map(|w| w.at))
    }

    /// Moves our window titled `title` so its top-left is `(x, y)`. The move is skipped when
    /// that point is on no monitor (a display that was unplugged). False if the window is not
    /// mapped yet.
    pub async fn move_to(&self, title: &str, x: i32, y: i32) -> Result<bool, DesktopError> {
        let Some(window) = self.own_window(title).await? else { return Ok(false) };
        if !on_a_monitor(&self.monitors().await?, x, y) {
            log::info!("saved meeting position is not on a monitor; leaving the window");
            return Ok(true);
        }
        self.move_window(&window, x, y).await?;
        Ok(true)
    }

    /// Asks Hyprland to reread its config. Runs the socket on this thread; call it from a
    /// worker, not the UI thread or an async worker.
    pub fn reload_blocking(&self) -> Result<(), DesktopError> {
        let reply = read_socket(&self.socket, "reload", self.timeout)?;
        let line = reply.lines().next().unwrap_or("").trim();
        if line.is_empty() || line == "ok" { Ok(()) } else { Err(DesktopError::Hyprland(brief(line).to_string())) }
    }

    async fn own_window(&self, title: &str) -> Result<Option<MappedWindow>, DesktopError> {
        own_window(&self.command("j/clients").await?, title, self.pid)
    }

    async fn monitors(&self) -> Result<Vec<Monitor>, DesktopError> {
        parse_monitors(&self.command("j/monitors").await?)
    }

    async fn move_window(&self, window: &MappedWindow, x: i32, y: i32) -> Result<(), DesktopError> {
        // A tiled window ignores a move. Float first when the rules have not
        // (a loaded rules file floats it on map; this covers the run before that).
        let Some((float_classic, float_lua)) = float_commands(&window.id) else {
            return Err(DesktopError::Hyprland("window address is not usable".into()));
        };
        let Some((move_classic, move_lua)) = move_commands(&window.id, x, y) else {
            return Err(DesktopError::Hyprland("window address is not usable".into()));
        };
        if !window.floating {
            self.dispatch_either(&float_classic, &float_lua).await?;
        }
        self.dispatch_either(&move_classic, &move_lua).await
    }

    async fn focus_address(&self, id: &str) -> Result<(), DesktopError> {
        let Some((classic, lua)) = focus_commands(id) else {
            return Err(DesktopError::Hyprland("window address is not usable".into()));
        };
        self.dispatch_either(&classic, &lua).await
    }

    /// Sends `classic` or `lua`. One [`LUA_PROBE`] per call until the reply is exactly `ok`
    /// (Lua) or `Invalid dispatcher` (classic). Anything else is not cached, so the next
    /// dispatch probes again. A call probes at most once, then either dispatches or returns.
    async fn dispatch_either(&self, classic: &str, lua: &str) -> Result<(), DesktopError> {
        match self.dispatch_kind().await? {
            DispatchKind::Lua => self.dispatch_raw(lua).await,
            DispatchKind::Classic => self.dispatch_raw(classic).await,
        }
    }

    async fn dispatch_kind(&self) -> Result<DispatchKind, DesktopError> {
        if let Some(kind) = *self.dispatch.lock().expect("dispatch kind") {
            return Ok(kind);
        }
        let body = self.command(&format!("dispatch {LUA_PROBE}")).await?;
        let kind = match probe_reply(&body) {
            Some(kind) => kind,
            None => {
                let line = body.lines().next().unwrap_or("").trim();
                return Err(DesktopError::Hyprland(format!("dispatch probe inconclusive: {}", brief(line))));
            }
        };
        *self.dispatch.lock().expect("dispatch kind") = Some(kind);
        log::info!("Hyprland dispatch syntax: {kind:?}");
        Ok(kind)
    }

    #[cfg(test)]
    fn for_test(socket: PathBuf) -> Self {
        Self { socket, pid: 1, timeout: Duration::from_millis(200), dispatch: std::sync::Mutex::new(None) }
    }

    async fn dispatch_raw(&self, args: &str) -> Result<(), DesktopError> {
        let reply = self.command(&format!("dispatch {args}")).await?;
        let line = reply.lines().next().unwrap_or("").trim();
        if line.is_empty() || line == "ok" {
            Ok(())
        } else {
            Err(DesktopError::Hyprland(format!("dispatch failed: {}", brief(line))))
        }
    }

    /// Socket round-trip on the runtime's blocking pool, so an async worker is not stuck on it.
    async fn command(&self, request: &str) -> Result<String, DesktopError> {
        let socket = self.socket.clone();
        let request = request.to_string();
        let timeout = self.timeout;
        tokio::task::spawn_blocking(move || read_socket(&socket, &request, timeout))
            .await
            .map_err(|e| DesktopError::Hyprland(format!("socket task: {e}")))?
    }
}

/// `Some(Lua)` only when the first line is exactly `ok`. `Some(Classic)` only when it is
/// exactly `Invalid dispatcher`. Empty or any other text is probed again next time.
fn probe_reply(body: &str) -> Option<DispatchKind> {
    let line = body.lines().next().unwrap_or("").trim();
    if line == "ok" {
        Some(DispatchKind::Lua)
    } else if line == "Invalid dispatcher" {
        Some(DispatchKind::Classic)
    } else {
        None
    }
}

/// Writes one command and reads the reply. The request is a fixed command or an address
/// and integers checked by [`is_window_address`]; it never contains a window title.
fn read_socket(socket: &Path, request: &str, timeout: Duration) -> Result<String, DesktopError> {
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket).map_err(|e| DesktopError::Hyprland(format!("socket: {e}")))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    stream.write_all(request.as_bytes()).map_err(|e| DesktopError::Hyprland(format!("socket write: {e}")))?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len().saturating_add(n) > MAX_REPLY {
                    return Err(DesktopError::Hyprland("reply is too large".into()));
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                if buf.is_empty() {
                    return Err(DesktopError::Hyprland("no answer from the socket".into()));
                }
                // The reply arrived and the server left the socket open.
                break;
            }
            Err(e) => return Err(DesktopError::Hyprland(format!("socket read: {e}"))),
        }
    }
    String::from_utf8(buf).map_err(|_| DesktopError::Hyprland("reply was not utf-8".into()))
}

fn socket_path() -> Result<PathBuf, DesktopError> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .ok_or_else(|| DesktopError::Hyprland("XDG_RUNTIME_DIR is not an absolute path".into()))?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| DesktopError::Hyprland("HYPRLAND_INSTANCE_SIGNATURE is not set".into()))?;
    socket_path_from(&runtime, &sig)
}

fn socket_path_from(runtime_dir: &Path, signature: &str) -> Result<PathBuf, DesktopError> {
    if !runtime_dir.is_absolute() {
        return Err(DesktopError::Hyprland("runtime dir is not absolute".into()));
    }
    if !signature_ok(signature) {
        return Err(DesktopError::Hyprland("HYPRLAND_INSTANCE_SIGNATURE is not a single path segment".into()));
    }
    Ok(runtime_dir.join("hypr").join(signature).join(".socket.sock"))
}

fn signature_ok(signature: &str) -> bool {
    !signature.is_empty()
        && signature.len() < 128
        && signature.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn logical_len(px: i32, scale: f64) -> i32 {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    (px as f64 / scale).round() as i32
}

/// Top-left of a window of `window` size, centred on `monitor`'s work area, `margin_bottom`
/// logical pixels above the bottom of that area (above a bottom bar and the Dock gap).
/// A top bar (Waybar on Omarchy) does not move this bottom edge.
pub fn pill_top_left(monitor: &Monitor, window: (i32, i32), margin_bottom: i32) -> (i32, i32) {
    let (logical_w, logical_h) = monitor.logical_size();
    let left = monitor.reserved.left.max(0);
    let right = monitor.reserved.right.max(0);
    let bottom = monitor.reserved.bottom.max(0);
    let span = logical_w.saturating_sub(left).saturating_sub(right);
    let x = monitor.x.saturating_add(left) + (span - window.0) / 2;
    let bottom_edge = monitor.y.saturating_add(logical_h).saturating_sub(bottom);
    let y = bottom_edge - window.1 - margin_bottom;
    (x, y)
}

pub fn focused_monitor(monitors: &[Monitor]) -> Option<&Monitor> {
    monitors.iter().find(|m| m.focused).or_else(|| monitors.first())
}

pub fn on_a_monitor(monitors: &[Monitor], x: i32, y: i32) -> bool {
    monitors.iter().any(|m| m.contains_point(x, y))
}

pub fn parse_active_window(body: &str) -> Result<Option<WindowRef>, DesktopError> {
    let body = body.trim();
    if body.is_empty() || body.eq_ignore_ascii_case("invalid") || body == "{}" {
        return Ok(None);
    }
    if !body.starts_with('{') {
        return Err(DesktopError::Hyprland("unexpected activewindow reply".into()));
    }
    let client: RawClient =
        serde_json::from_str(body).map_err(|_| DesktopError::Hyprland("could not parse the active window".into()))?;
    Ok(window_ref(&client))
}

/// Our window with this title. The pid is the one Hyprland recorded for the client (it cannot
/// be chosen by another process). The class is not required: GTK sets the Wayland app id from
/// the program name unless Tauri's `enableGTKAppId` is on, and this app does not set that.
pub fn own_window(body: &str, title: &str, pid: i64) -> Result<Option<MappedWindow>, DesktopError> {
    Ok(parse_clients(body)?.into_iter().find(|c| c.pid == pid && c.title == title).and_then(|c| mapped(&c)))
}

/// Classic dispatcher argument, and the Hyprland 0.56 Lua form (`hl.dsp.focus`).
pub fn focus_commands(address: &str) -> Option<(String, String)> {
    if !is_window_address(address) {
        return None;
    }
    Some((format!("focuswindow address:{address}"), format!("hl.dsp.focus({{ window = 'address:{address}' }})")))
}

/// Classic `movewindowpixel exact`, and `hl.dsp.window.move` (absolute: `relative` defaults to false).
pub fn move_commands(address: &str, x: i32, y: i32) -> Option<(String, String)> {
    if !is_window_address(address) {
        return None;
    }
    Some((
        format!("movewindowpixel exact {x} {y},address:{address}"),
        format!("hl.dsp.window.move({{ window = 'address:{address}', x = {x}, y = {y} }})"),
    ))
}

/// Classic `setfloating`, and `hl.dsp.window.float` with `action = 'set'`.
pub fn float_commands(address: &str) -> Option<(String, String)> {
    if !is_window_address(address) {
        return None;
    }
    Some((
        format!("setfloating address:{address}"),
        format!("hl.dsp.window.float({{ window = 'address:{address}', action = 'enable' }})"),
    ))
}

/// Harmless Lua dispatcher (`hl.dsp.no_op` in Hyprland 0.56). A classic config does not accept it.
const LUA_PROBE: &str = "hl.dsp.no_op()";

fn parse_clients(body: &str) -> Result<Vec<RawClient>, DesktopError> {
    let body = body.trim();
    if !body.starts_with('[') {
        return Err(DesktopError::Hyprland("unexpected clients reply".into()));
    }
    serde_json::from_str(body).map_err(|_| DesktopError::Hyprland("could not parse the window list".into()))
}

pub fn parse_monitors(body: &str) -> Result<Vec<Monitor>, DesktopError> {
    let body = body.trim();
    if !body.starts_with('[') {
        return Err(DesktopError::Hyprland("unexpected monitors reply".into()));
    }
    let raw: Vec<RawMonitor> =
        serde_json::from_str(body).map_err(|_| DesktopError::Hyprland("could not parse the monitor list".into()))?;
    Ok(raw.into_iter().map(Monitor::from).collect())
}

fn window_ref(client: &RawClient) -> Option<WindowRef> {
    if !is_window_address(&client.address) {
        return None;
    }
    Some(WindowRef { id: client.address.clone(), class: client.class.clone() })
}

fn mapped(client: &RawClient) -> Option<MappedWindow> {
    let id = window_ref(client)?;
    Some(MappedWindow {
        id: id.id,
        class: id.class,
        at: pair(&client.at),
        size: pair(&client.size),
        floating: client.floating,
    })
}

fn pair(v: &[f64]) -> (i32, i32) {
    let n = |i: usize| v.get(i).copied().unwrap_or(0.0).round() as i32;
    (n(0), n(1))
}

fn brief(line: &str) -> &str {
    let mut end = 200.min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

#[derive(Deserialize)]
struct RawClient {
    #[serde(default)]
    address: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    at: Vec<f64>,
    #[serde(default)]
    size: Vec<f64>,
    #[serde(default)]
    floating: bool,
    /// Process id Hyprland took from the client. Absent or null stays 0, which is not us.
    #[serde(default)]
    pid: i64,
}

#[derive(Deserialize)]
struct RawMonitor {
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: i32,
    #[serde(default)]
    height: i32,
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    reserved: Vec<i32>,
}

fn one() -> f64 {
    1.0
}

impl From<RawMonitor> for Monitor {
    fn from(raw: RawMonitor) -> Self {
        let z = raw.reserved;
        let at = |i: usize| z.get(i).copied().unwrap_or(0);
        // hyprctl: reserved is [left, top, right, bottom].
        Self {
            x: raw.x,
            y: raw.y,
            width: raw.width,
            height: raw.height,
            scale: raw.scale,
            focused: raw.focused,
            reserved: Reserved { left: at(0), top: at(1), right: at(2), bottom: at(3) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_detection_follows_the_desktop_name() {
        assert!(detect_hyprland(false, Some("Hyprland"), None));
        assert!(detect_hyprland(false, Some("ubuntu:Hyprland"), None));
        assert!(detect_hyprland(false, None, Some("abc123")));
        assert!(detect_hyprland(false, Some(""), Some("abc123")));
        assert!(!detect_hyprland(false, Some("KDE"), Some("abc123")));
        assert!(!detect_hyprland(false, Some("GNOME"), None));
        assert!(!detect_hyprland(true, Some("Hyprland"), Some("abc123")));
        assert!(!detect_hyprland(false, None, None));
        assert!(!detect_hyprland(false, None, Some("  ")));
    }

    #[test]
    fn addresses_are_hex_and_not_null() {
        assert!(is_window_address("0x558b0a0c5c40"));
        assert!(is_window_address("0xabc"));
        assert!(!is_window_address("0x0"));
        assert!(!is_window_address("0x000"));
        assert!(!is_window_address("558b0a0c5c40"));
        assert!(!is_window_address("0x12/34"));
        assert!(!is_window_address("0x12\n34"));
    }

    #[test]
    fn socket_path_stays_inside_the_instance_dir() {
        let ok = socket_path_from(Path::new("/run/user/1000"), "abc_123").unwrap();
        assert_eq!(ok, PathBuf::from("/run/user/1000/hypr/abc_123/.socket.sock"));
        assert!(socket_path_from(Path::new("/run/user/1000"), "../etc").is_err());
        assert!(socket_path_from(Path::new("/run/user/1000"), "a/b").is_err());
        assert!(socket_path_from(Path::new("/run/user/1000"), "").is_err());
        assert!(socket_path_from(Path::new("relative"), "abc").is_err());
    }

    #[test]
    fn active_window_keeps_the_address_and_drops_the_title() {
        let body = r#"{"address":"0x558b0a0c5c40","class":"kitty","title":"user typed this","at":[1,2],"size":[3,4]}"#;
        let w = parse_active_window(body).unwrap().unwrap();
        assert_eq!(w.id, "0x558b0a0c5c40");
        assert_eq!(w.class, "kitty");
        assert!(!format!("{w:?}").contains("user typed"));
        assert!(parse_active_window("{}").unwrap().is_none());
        assert!(parse_active_window(r#"{"address":"0x0","class":"kitty","title":"x"}"#).unwrap().is_none());
        let err = parse_active_window("not json, title stays here").unwrap_err();
        assert!(!err.to_string().contains("title stays"));
    }

    #[test]
    fn own_window_is_pid_and_title_even_when_the_class_is_tyst() {
        let body = r#"[
            {"address":"0x11","class":"kitty","title":"Tyst Dictation","pid":9,"at":[0,0],"size":[80,24],"floating":false},
            {"address":"0x22","class":"tyst","title":"Tyst Dictation","pid":42,"at":[10,20],"size":[560,168],"floating":true},
            {"address":"0x33","class":"tyst","title":"Tyst Meeting","pid":42,"at":[5,6],"size":[7,8],"floating":true}
        ]"#;
        // GTK's program name is `tyst` (StartupWMClass). That window is ours when the pid matches.
        let w = own_window(body, "Tyst Dictation", 42).unwrap().unwrap();
        assert_eq!(w.id, "0x22");
        assert_eq!(w.class, "tyst");
        assert_eq!(w.at, (10, 20));
        assert_eq!(w.size, (560, 168));
        assert!(w.floating);
        assert!(!format!("{w:?}").contains("Tyst Dictation"));
        assert_eq!(own_window(body, "Tyst Meeting", 42).unwrap().unwrap().id, "0x33");
        assert_eq!(own_window(body, "Tyst Dictation", 9).unwrap().unwrap().id, "0x11");
        assert!(own_window(body, "Tyst Dictation", 7).unwrap().is_none());
        assert!(own_window(body, "missing", 42).unwrap().is_none());
    }

    #[test]
    fn lua_dispatch_matches_hyprland_0_56_and_classic_is_the_fallback() {
        let (classic, lua) = focus_commands("0xabc").unwrap();
        assert_eq!(classic, "focuswindow address:0xabc");
        assert_eq!(lua, "hl.dsp.focus({ window = 'address:0xabc' })");
        let (classic, lua) = move_commands("0xabc", 680, 2543).unwrap();
        assert_eq!(classic, "movewindowpixel exact 680 2543,address:0xabc");
        assert_eq!(lua, "hl.dsp.window.move({ window = 'address:0xabc', x = 680, y = 2543 })");
        let (classic, lua) = float_commands("0xabc").unwrap();
        assert_eq!(classic, "setfloating address:0xabc");
        assert_eq!(lua, "hl.dsp.window.float({ window = 'address:0xabc', action = 'enable' })");
        assert!(focus_commands("0x0").is_none());
    }

    /// One directory per test so two sockets never share a path. Removed when dropped.
    struct FakeSocket {
        hypr: Hyprland,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        dir: PathBuf,
    }

    impl Drop for FakeSocket {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn unique_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tyst-hypr-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Speaks the Hyprland socket protocol: read one request until the client closes its write
    /// side, then write `reply` and close. Each connection is its own thread so a slow reply
    /// does not block the next one.
    fn fake_socket(reply: impl Fn(&str) -> String + Send + Sync + 'static) -> FakeSocket {
        let dir = unique_dir();
        let path = dir.join("hypr.sock");
        let seen = serve(&path, reply);
        FakeSocket { hypr: Hyprland::for_test(path), seen, dir }
    }

    fn serve(
        path: &Path,
        reply: impl Fn(&str) -> String + Send + Sync + 'static,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).expect("bind fake hyprland socket");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let reply = std::sync::Arc::new(reply);
        std::thread::spawn(move || {
            for _ in 0..16 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let record = record.clone();
                let reply = reply.clone();
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 2048];
                    loop {
                        match stream.read(&mut tmp) {
                            Ok(0) => break,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => return,
                        }
                    }
                    let req = String::from_utf8_lossy(&buf).into_owned();
                    record.lock().expect("requests").push(req.clone());
                    let _ = stream.write_all(reply(&req).as_bytes());
                });
            }
        });
        seen
    }

    #[tokio::test]
    async fn dispatch_either_probes_then_uses_classic_when_lua_is_unknown() {
        let fake =
            fake_socket(|req| if req.contains("hl.dsp.no_op") { "Invalid dispatcher\n".into() } else { "ok\n".into() });
        let hypr = &fake.hypr;
        let seen = &fake.seen;
        let (classic, lua) = focus_commands("0xabc").unwrap();
        hypr.dispatch_either(&classic, &lua).await.expect("classic focus");
        hypr.dispatch_either(&classic, &lua).await.expect("cached classic");
        let got = seen.lock().expect("requests").clone();
        assert_eq!(
            got,
            vec!["dispatch hl.dsp.no_op()".to_string(), format!("dispatch {classic}"), format!("dispatch {classic}"),]
        );
    }

    #[tokio::test]
    async fn dispatch_either_uses_lua_after_the_probe_and_does_not_send_classic() {
        let fake = fake_socket(|req| {
            if req.contains("focuswindow") {
                "hl.dispatch(...): ')' expected near 'address'\n".into()
            } else {
                "ok\n".into()
            }
        });
        let hypr = &fake.hypr;
        let seen = &fake.seen;
        let (classic, lua) = focus_commands("0xabc").unwrap();
        hypr.dispatch_either(&classic, &lua).await.expect("lua focus");
        let got = seen.lock().expect("requests").clone();
        assert_eq!(got, vec!["dispatch hl.dsp.no_op()".to_string(), format!("dispatch {lua}")]);
        assert!(!got.iter().any(|r| r.contains("focuswindow")));
    }

    #[tokio::test]
    async fn dispatch_either_returns_a_real_error_without_trying_the_other_grammar() {
        let fake =
            fake_socket(|req| if req.contains("hl.dsp.no_op") { "ok\n".into() } else { "no such window\n".into() });
        let hypr = &fake.hypr;
        let seen = &fake.seen;
        let (classic, lua) = focus_commands("0xabc").unwrap();
        let err = hypr.dispatch_either(&classic, &lua).await.expect_err("window is gone");
        assert!(err.to_string().contains("no such window"), "{err}");
        let got = seen.lock().expect("requests").clone();
        assert_eq!(got, vec!["dispatch hl.dsp.no_op()".to_string(), format!("dispatch {lua}")]);
    }

    #[test]
    fn probe_reply_is_only_exact_ok_or_invalid_dispatcher() {
        assert_eq!(probe_reply("ok"), Some(DispatchKind::Lua));
        assert_eq!(probe_reply("ok\n"), Some(DispatchKind::Lua));
        assert_eq!(probe_reply("Invalid dispatcher"), Some(DispatchKind::Classic));
        assert_eq!(probe_reply("Invalid dispatcher\n"), Some(DispatchKind::Classic));
        assert_eq!(probe_reply(""), None);
        assert_eq!(probe_reply("ok please"), None);
        assert_eq!(probe_reply("no such window"), None);
        assert_eq!(probe_reply("hl.dispatch(...): ')' expected near 'address'"), None);
    }

    #[tokio::test]
    async fn empty_or_unexpected_probe_is_not_cached() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let n = std::sync::Arc::new(AtomicU32::new(0));
        let ticks = n.clone();
        let fake = fake_socket(move |req| {
            if !req.contains("hl.dsp.no_op") {
                return "ok\n".into();
            }
            match ticks.fetch_add(1, Ordering::Relaxed) {
                0 => String::new(),
                1 => "please reboot\n".into(),
                _ => "Invalid dispatcher\n".into(),
            }
        });
        let (classic, lua) = focus_commands("0xabc").unwrap();
        let empty = fake.hypr.dispatch_either(&classic, &lua).await.expect_err("empty probe");
        assert!(empty.to_string().contains("inconclusive"), "{empty}");
        let odd = fake.hypr.dispatch_either(&classic, &lua).await.expect_err("unexpected probe");
        assert!(odd.to_string().contains("inconclusive"), "{odd}");
        fake.hypr.dispatch_either(&classic, &lua).await.expect("classic after a real reply");
        let got = fake.seen.lock().expect("requests").clone();
        assert_eq!(
            got,
            vec![
                "dispatch hl.dsp.no_op()".to_string(),
                "dispatch hl.dsp.no_op()".to_string(),
                "dispatch hl.dsp.no_op()".to_string(),
                format!("dispatch {classic}"),
            ]
        );
    }

    #[tokio::test]
    async fn socket_error_and_timeout_are_not_cached() {
        let dir = unique_dir();
        let path = dir.join("hypr.sock");
        let hypr = Hyprland::for_test(path.clone());
        let (classic, lua) = focus_commands("0xabc").unwrap();
        let missing = hypr.dispatch_either(&classic, &lua).await.expect_err("no socket");
        assert!(missing.to_string().contains("socket"), "{missing}");

        let slow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = slow.clone();
        let seen = serve(&path, move |req| {
            if req.contains("hl.dsp.no_op") && gate.swap(false, std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(400));
                return "ok\n".into();
            }
            if req.contains("hl.dsp.no_op") {
                return "Invalid dispatcher\n".into();
            }
            "ok\n".into()
        });
        let timed = hypr.dispatch_either(&classic, &lua).await.expect_err("probe timed out");
        assert!(timed.to_string().contains("no answer") || timed.to_string().contains("socket"), "{timed}");
        hypr.dispatch_either(&classic, &lua).await.expect("classic after the timeout");
        let got = seen.lock().expect("requests").clone();
        assert!(got.iter().filter(|r| r.contains("hl.dsp.no_op")).count() >= 2, "{got:?}");
        assert!(got.last().is_some_and(|r| r.contains("focuswindow")), "{got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn monitor(x: i32, y: i32, width: i32, height: i32, scale: f64, reserved: Reserved, focused: bool) -> Monitor {
        Monitor { x, y, width, height, scale, focused, reserved }
    }

    #[test]
    fn pill_sits_bottom_centre_of_the_focused_monitor() {
        let bare = monitor(0, 0, 1920, 1080, 1.0, Reserved::default(), true);
        // 560×168 window, 48 px gap: (1920-560)/2 = 680, 1080-168-48 = 864.
        assert_eq!(pill_top_left(&bare, (560, 168), 48), (680, 864));
        // Omarchy's Waybar is a top exclusive zone. `reserved` is [left, top, right, bottom],
        // so [0, 34, 0, 0] must not lift the pill the way a bottom bar would.
        let waybar = monitor(0, 0, 1920, 1080, 1.0, Reserved { top: 34, ..Reserved::default() }, true);
        assert_eq!(pill_top_left(&waybar, (560, 168), 48), (680, 864));
        let bottom_bar = monitor(0, 0, 1920, 1080, 1.0, Reserved { bottom: 40, ..Reserved::default() }, true);
        assert_eq!(pill_top_left(&bottom_bar, (560, 168), 48), (680, 824));
        // [left, top, right, bottom]: a left bar shifts the centre, it does not change the bottom edge.
        let left_bar = monitor(0, 0, 1920, 1080, 1.0, Reserved { left: 80, ..Reserved::default() }, true);
        assert_eq!(pill_top_left(&left_bar, (560, 168), 48), (720, 864));
        // 3840×2160 at scale 2 is a 1920×1080 layout. The second monitor starts at x=1920.
        let hidpi = monitor(1920, 0, 3840, 2160, 2.0, Reserved::default(), true);
        assert_eq!(pill_top_left(&hidpi, (560, 168), 48), (2600, 864));
        let monitors = [
            monitor(0, 0, 1920, 1080, 1.0, Reserved::default(), false),
            monitor(1920, 0, 1920, 1080, 1.0, Reserved::default(), true),
        ];
        assert_eq!(focused_monitor(&monitors).unwrap().x, 1920);
        assert!(on_a_monitor(&monitors, 2000, 10));
        assert!(!on_a_monitor(&monitors, 5000, 10));
    }

    #[test]
    fn monitors_json_reserved_is_left_top_right_bottom() {
        let body = r#"[{"name":"DP-1","width":1920,"height":1080,"x":0,"y":0,"scale":1.0,"focused":true,"reserved":[0,34,0,0],"refreshRate":60.0}]"#;
        let monitors = parse_monitors(body).unwrap();
        assert_eq!(monitors[0].reserved, Reserved { top: 34, ..Reserved::default() });
        assert_eq!(monitors[0].logical_size(), (1920, 1080));
        assert_eq!(pill_top_left(&monitors[0], (560, 168), 48), (680, 864));
    }
}
