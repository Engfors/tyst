//! Desktop glue for dictation (SPEC 8.4, 10): remembering the window to paste into, placing and
//! focusing the pill, and the paste itself (clipboard -> paste keystroke -> clipboard restored).
//!
//! Linux: KWin scripting (KDE) or Hyprland IPC finds and re-activates the window and places the
//! pill; other desktops hand focus back on their own. The RemoteDesktop portal types the keystroke
//! (with `ydotool` as a fallback). Hyprland's portal has no RemoteDesktop, so paste there is
//! `wtype` and then `ydotool`. Terminals get Ctrl+Shift+V. macOS: the frontmost app is
//! re-activated and Cmd+V posted. Nothing here logs clipboard text or window titles.

use std::time::Duration;

use tauri::AppHandle;
use tauri::async_runtime::Mutex as AsyncMutex;
use tyst_platform::clipboard::Clipboard;

use crate::windows;

/// How long the target window gets to take focus before the keystroke.
const FOCUS_SETTLE: Duration = Duration::from_millis(80);
/// How long the pasted text stays on the clipboard before the old content comes back.
const RESTORE_AFTER: Duration = Duration::from_millis(500);

/// The window that was focused when dictation started, on the compositor that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(target_os = "linux")]
pub enum TrackedWindow {
    Kwin(tyst_platform::kwin::WindowRef),
    Hyprland(tyst_platform::hyprland::WindowRef),
}

#[cfg(target_os = "linux")]
impl TrackedWindow {
    fn class(&self) -> &str {
        match self {
            Self::Kwin(w) => w.class.as_str(),
            Self::Hyprland(w) => w.class.as_str(),
        }
    }
}

/// Where the dictated text goes: the window that was active when dictation started.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// KWin or Hyprland's window id and class.
    #[cfg(target_os = "linux")]
    pub window: Option<TrackedWindow>,
    /// Process id of the frontmost app (macOS).
    #[cfg(target_os = "macos")]
    pub pid: Option<i32>,
}

impl Target {
    /// The window class (app id), where known.
    pub fn class(&self) -> Option<&str> {
        #[cfg(target_os = "linux")]
        return self.window.as_ref().map(TrackedWindow::class);
        #[allow(unreachable_code)]
        None
    }
}

/// True where KWin or Hyprland places the windows (Wayland clients cannot).
pub fn compositor_manages_windows() -> bool {
    #[cfg(target_os = "linux")]
    {
        return tyst_platform::kwin::is_kde() || tyst_platform::hyprland::is_hyprland();
    }
    #[allow(unreachable_code)]
    false
}

/// The meeting window's top-left in the compositor's coordinates, if this desktop records one.
#[cfg(target_os = "linux")]
pub fn stored_meeting_origin(meetings: &crate::config::MeetingSettings) -> Option<(i32, i32)> {
    if tyst_platform::kwin::is_kde() {
        return meetings.kwin_position;
    }
    if tyst_platform::hyprland::is_hyprland() {
        return meetings.hypr_position;
    }
    None
}

/// True where the app places the pill itself; false where the compositor does it.
pub fn places_windows() -> bool {
    !compositor_manages_windows()
}

pub struct Desktop {
    clipboard: Clipboard,
    #[cfg(target_os = "linux")]
    kwin: AsyncMutex<Option<std::sync::Arc<tyst_platform::kwin::KWin>>>,
    #[cfg(target_os = "linux")]
    hyprland: AsyncMutex<Option<std::sync::Arc<tyst_platform::hyprland::Hyprland>>>,
    #[cfg(target_os = "linux")]
    keyboard: AsyncMutex<Option<tyst_platform::portal::PortalKeyboard>>,
    #[cfg(not(target_os = "linux"))]
    _keyboard: AsyncMutex<()>,
}

impl Default for Desktop {
    fn default() -> Self {
        Self {
            clipboard: Clipboard::new(),
            #[cfg(target_os = "linux")]
            kwin: AsyncMutex::new(None),
            #[cfg(target_os = "linux")]
            hyprland: AsyncMutex::new(None),
            #[cfg(target_os = "linux")]
            keyboard: AsyncMutex::new(None),
            #[cfg(not(target_os = "linux"))]
            _keyboard: AsyncMutex::new(()),
        }
    }
}

/// What a paste did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pasted {
    /// The keystroke was sent.
    Typed,
    /// Only the clipboard holds the text: the keystroke could not be sent.
    ClipboardOnly,
    /// Only the clipboard holds the text: the user refused keyboard access for paste.
    Denied,
}

/// `text` on one line: line and paragraph breaks become spaces.
fn single_line(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return std::borrow::Cow::Borrowed(text);
    }
    let words: Vec<&str> =
        text.split(['\n', '\r', '\u{2028}', '\u{2029}']).map(str::trim).filter(|l| !l.is_empty()).collect();
    std::borrow::Cow::Owned(words.join(" "))
}

impl Desktop {
    #[cfg(target_os = "linux")]
    async fn kwin(&self) -> Option<std::sync::Arc<tyst_platform::kwin::KWin>> {
        if !tyst_platform::kwin::is_kde() {
            return None;
        }
        let mut k = self.kwin.lock().await;
        if k.is_none() {
            match tyst_platform::kwin::KWin::connect().await {
                Ok(c) => *k = Some(std::sync::Arc::new(c)),
                Err(e) => log::warn!("KWin scripting unavailable: {e}"),
            }
        }
        k.clone()
    }

    #[cfg(target_os = "linux")]
    async fn hyprland(&self) -> Option<std::sync::Arc<tyst_platform::hyprland::Hyprland>> {
        if !tyst_platform::hyprland::is_hyprland() {
            return None;
        }
        let mut h = self.hyprland.lock().await;
        if h.is_none() {
            match tyst_platform::hyprland::Hyprland::connect(crate::shortcuts::APP_ID) {
                Ok(c) => *h = Some(std::sync::Arc::new(c)),
                Err(e) => log::warn!("Hyprland IPC unavailable: {e}"),
            }
        }
        h.clone()
    }

    /// The window to paste into later (call before the pill shows).
    pub async fn target(&self) -> Target {
        #[cfg(target_os = "linux")]
        {
            let window = if let Some(k) = self.kwin().await {
                k.active_window()
                    .await
                    .unwrap_or_else(|e| {
                        log::warn!("active window: {e}");
                        None
                    })
                    .map(TrackedWindow::Kwin)
            } else if let Some(h) = self.hyprland().await {
                h.active_window()
                    .await
                    .unwrap_or_else(|e| {
                        log::warn!("active window: {e}");
                        None
                    })
                    .map(TrackedWindow::Hyprland)
            } else {
                None
            };
            Target { window }
        }
        #[cfg(target_os = "macos")]
        {
            Target { pid: tyst_platform::macos_input::frontmost_app() }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Target::default()
    }

    /// Shows the pill. On KDE and Hyprland the compositor then moves it to the bottom centre of
    /// the focused screen and, with `focus`, activates it.
    pub async fn show_pill(&self, app: &AppHandle, focus: bool) {
        let shown = windows::show_pill(app, focus).is_some();
        #[cfg(target_os = "linux")]
        if shown {
            // A freshly shown window is mapped a moment later; try again briefly.
            for _ in 0..10 {
                let placed = if let Some(k) = self.kwin().await {
                    k.place_bottom_center(windows::PILL_TITLE, windows::PILL_BOTTOM as i32, focus).await
                } else if let Some(h) = self.hyprland().await {
                    h.place_bottom_center(windows::PILL_TITLE, windows::PILL_BOTTOM as i32, focus).await
                } else {
                    break;
                };
                match placed {
                    Ok(true) => break,
                    Ok(false) => tokio_sleep(Duration::from_millis(20)).await,
                    Err(e) => {
                        log::warn!("placing the pill: {e}");
                        break;
                    }
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = shown;
    }

    /// KDE and Hyprland: remembers where the meeting window is, before it hides.
    #[cfg(target_os = "linux")]
    pub async fn remember_meeting_position(&self, app: &AppHandle) {
        let (hypr, pos) = if let Some(k) = self.kwin().await {
            (false, k.position(windows::MEETING_TITLE).await)
        } else if let Some(h) = self.hyprland().await {
            (true, h.position(windows::MEETING_TITLE).await)
        } else {
            return;
        };
        match pos {
            Ok(Some(p)) => {
                use tauri::Manager;
                let state = app.state::<crate::state::AppState>();
                let mut cfg = state.config.lock().expect("config lock");
                if hypr {
                    cfg.meetings.hypr_position = Some(p);
                } else {
                    cfg.meetings.kwin_position = Some(p);
                }
            }
            Ok(None) => {}
            Err(e) => log::warn!("reading the meeting window position: {e}"),
        }
    }

    /// KDE and Hyprland: puts the meeting window back where it was, once it is mapped.
    #[cfg(target_os = "linux")]
    pub async fn restore_meeting_position(&self, (x, y): (i32, i32)) {
        // A freshly shown window is mapped a moment later; try again briefly.
        for _ in 0..25 {
            let moved = if let Some(k) = self.kwin().await {
                k.move_to(windows::MEETING_TITLE, x, y).await
            } else if let Some(h) = self.hyprland().await {
                h.move_to(windows::MEETING_TITLE, x, y).await
            } else {
                return;
            };
            match moved {
                Ok(true) => break,
                Ok(false) => tokio_sleep(Duration::from_millis(20)).await,
                Err(e) => {
                    log::warn!("placing the meeting window: {e}");
                    break;
                }
            }
        }
    }

    /// Hyprland: focuses the meeting window so the name field can take keys. `no_initial_focus`
    /// only skips the focus Hyprland would give the window on map; a later focus request is how
    /// the name field is typed into. `no_focus` is not set, because that would refuse this.
    #[cfg(target_os = "linux")]
    pub async fn focus_meeting_window(&self) {
        let Some(h) = self.hyprland().await else { return };
        for _ in 0..10 {
            match h.focus_title(windows::MEETING_TITLE).await {
                Ok(true) => return,
                Ok(false) => tokio_sleep(Duration::from_millis(20)).await,
                Err(e) => {
                    log::warn!("focusing the meeting window: {e}");
                    return;
                }
            }
        }
    }

    /// Gives the keyboard back to the target window.
    pub async fn return_to(&self, target: &Target) {
        #[cfg(target_os = "linux")]
        if let Some(window) = &target.window {
            let activated = match window {
                TrackedWindow::Kwin(w) => match self.kwin().await {
                    Some(k) => k.activate(&w.id).await,
                    None => return,
                },
                TrackedWindow::Hyprland(w) => match self.hyprland().await {
                    Some(h) => h.activate(&w.id).await,
                    None => return,
                },
            };
            match activated {
                Ok(true) => {}
                Ok(false) => log::info!("the dictation target window is gone"),
                Err(e) => log::warn!("activating the target window: {e}"),
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(pid) = target.pid
            && !tyst_platform::macos_input::activate_app(pid)
        {
            log::info!("the dictation target app is gone");
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = target;
    }

    /// Puts `text` on the clipboard for good (Ctrl+C in the pill).
    pub fn copy(&self, text: &str) -> Result<(), String> {
        self.clipboard.set_text(text, false).map_err(|e| e.to_string())
    }

    /// Pastes `text` into `target`: clipboard, focus back, paste keystroke, then the previous
    /// clipboard text again after half a second when `restore` is on (SPEC 8.4). Terminals in
    /// `terminals` get Ctrl+Shift+V. `keyboard_token` is the portal's restore token; the new one
    /// is returned for the settings.
    pub async fn paste(
        &self,
        target: &Target,
        text: &str,
        restore: bool,
        terminals: &[String],
        keyboard_token: Option<String>,
    ) -> Result<(Pasted, Option<String>), String> {
        let previous = if restore { self.clipboard.text() } else { None };
        let terminal = target.class().is_some_and(|c| crate::config::is_terminal(c, terminals));
        // A newline pasted into a terminal runs what came before it as a command.
        let text = if terminal { single_line(text) } else { std::borrow::Cow::Borrowed(text) };
        self.clipboard.set_text(&text, restore).map_err(|e| e.to_string())?;
        self.return_to(target).await;
        tokio_sleep(FOCUS_SETTLE).await;
        let (pasted, token) = self.keystroke(terminal, keyboard_token).await;
        if pasted == Pasted::Typed
            && let Some(prev) = previous
        {
            tokio_sleep(RESTORE_AFTER).await;
            if let Err(e) = self.clipboard.set_text(&prev, false) {
                log::warn!("restoring the clipboard: {e}");
            }
        }
        Ok((pasted, token))
    }

    #[cfg(target_os = "linux")]
    async fn keystroke(&self, terminal: bool, token: Option<String>) -> (Pasted, Option<String>) {
        // xdg-desktop-portal-hyprland implements GlobalShortcuts and not RemoteDesktop, so the
        // portal cannot type. wtype (virtual keyboard) then ydotool.
        if tyst_platform::hyprland::is_hyprland() {
            return keystroke_hyprland(terminal, token);
        }
        use tyst_platform::DesktopError;
        let mut kb = self.keyboard.lock().await;
        let mut missing = false;
        if kb.is_none() {
            match tyst_platform::portal::PortalKeyboard::new(token.clone()).await {
                Ok(k) => *kb = Some(k),
                Err(e) => {
                    missing = matches!(e, DesktopError::PortalMissing(_));
                    log::warn!("keyboard portal: {e}");
                }
            }
        }
        if let Some(k) = kb.as_mut() {
            match k.paste(terminal).await {
                Ok(()) => return (Pasted::Typed, k.restore_token().map(String::from)),
                Err(e) => {
                    missing = matches!(e, DesktopError::PortalMissing(_));
                    log::warn!("paste through the keyboard portal: {e}");
                }
            }
        }
        // ydotool is the manual fallback for desktops without the portal (SPEC 10.2), never a
        // way around a refused consent dialog.
        if !missing {
            return (Pasted::Denied, token);
        }
        match tyst_platform::portal::ydotool_paste(terminal) {
            Ok(()) => (Pasted::Typed, token),
            Err(e) => {
                log::warn!("paste through ydotool: {e}");
                (Pasted::ClipboardOnly, token)
            }
        }
    }

    #[cfg(target_os = "macos")]
    async fn keystroke(&self, _terminal: bool, token: Option<String>) -> (Pasted, Option<String>) {
        // Asks macOS to show its Accessibility dialog when the answer is no.
        if !tyst_platform::macos_input::trusted(true) {
            log::warn!("paste: no Accessibility permission");
            return (Pasted::Denied, token);
        }
        match tyst_platform::macos_input::paste() {
            Ok(()) => (Pasted::Typed, token),
            Err(e) => {
                log::warn!("paste: {e}");
                (Pasted::ClipboardOnly, token)
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    async fn keystroke(&self, _terminal: bool, token: Option<String>) -> (bool, Option<String>) {
        (false, token)
    }

    /// Asks for keyboard access now (the KDE consent dialog), for onboarding and settings.
    /// Returns the restore token to keep. Hyprland has no such dialog; paste uses wtype.
    pub async fn setup_keyboard(&self, token: Option<String>) -> Result<Option<String>, String> {
        #[cfg(target_os = "linux")]
        {
            if tyst_platform::hyprland::is_hyprland() {
                return Err(
                    "Hyprland has no remote-desktop portal. Paste uses wtype (ydotool if wtype is missing).".into()
                );
            }
            let mut kb = self.keyboard.lock().await;
            if kb.is_none() {
                *kb = Some(tyst_platform::portal::PortalKeyboard::new(token).await.map_err(|e| e.to_string())?);
            }
            let k = kb.as_mut().expect("set above");
            k.open().await.map_err(|e| e.to_string())?;
            Ok(k.restore_token().map(String::from))
        }
        #[cfg(not(target_os = "linux"))]
        Ok(token)
    }
}

/// Hyprland paste: `wtype` (Ctrl+V, Ctrl+Shift+V in a terminal), then `ydotool`.
#[cfg(target_os = "linux")]
fn keystroke_hyprland(terminal: bool, token: Option<String>) -> (Pasted, Option<String>) {
    match tyst_platform::portal::wtype_paste(terminal) {
        Ok(()) => return (Pasted::Typed, token),
        Err(e) => log::warn!("paste through wtype: {e}"),
    }
    match tyst_platform::portal::ydotool_paste(terminal) {
        Ok(()) => (Pasted::Typed, token),
        Err(e) => {
            log::warn!("paste through ydotool: {e}");
            (Pasted::ClipboardOnly, token)
        }
    }
}

async fn tokio_sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_paste_is_one_line() {
        assert_eq!(single_line("git status"), "git status");
        assert_eq!(single_line("rm -rf build\nmake\r\n\n  ls\u{2028}x"), "rm -rf build make ls x");
    }
}
