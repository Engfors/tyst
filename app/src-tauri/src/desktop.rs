//! Desktop glue for dictation (SPEC 8.4, 10): remembering the window to paste into, placing and
//! focusing the pill, and the paste itself (clipboard -> paste keystroke -> clipboard restored).
//!
//! Linux: KWin scripting finds and re-activates the window and places the pill (KDE only; other
//! desktops hand focus back on their own), the RemoteDesktop portal types the keystroke (with
//! `ydotool` as a fallback), and terminals get Ctrl+Shift+V. macOS: the frontmost app is
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

/// Where the dictated text goes: the window that was active when dictation started.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// KWin's window id and resource class (KDE).
    #[cfg(target_os = "linux")]
    pub window: Option<tyst_platform::kwin::WindowRef>,
    /// Process id of the frontmost app (macOS).
    #[cfg(target_os = "macos")]
    pub pid: Option<i32>,
}

impl Target {
    /// The window class (app id), where known.
    pub fn class(&self) -> Option<&str> {
        #[cfg(target_os = "linux")]
        return self.window.as_ref().map(|w| w.class.as_str());
        #[allow(unreachable_code)]
        None
    }
}

/// True where the app places the pill itself; false on KDE, where KWin does it.
pub fn places_windows() -> bool {
    #[cfg(target_os = "linux")]
    return !tyst_platform::kwin::is_kde();
    #[allow(unreachable_code)]
    true
}

pub struct Desktop {
    clipboard: Clipboard,
    #[cfg(target_os = "linux")]
    kwin: AsyncMutex<Option<std::sync::Arc<tyst_platform::kwin::KWin>>>,
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

    /// The window to paste into later (call before the pill shows).
    pub async fn target(&self) -> Target {
        #[cfg(target_os = "linux")]
        {
            let window = match self.kwin().await {
                Some(k) => k.active_window().await.unwrap_or_else(|e| {
                    log::warn!("active window: {e}");
                    None
                }),
                None => None,
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

    /// Shows the pill; on KDE KWin then moves it to the bottom centre of the active screen and,
    /// with `focus`, activates it.
    pub async fn show_pill(&self, app: &AppHandle, focus: bool) {
        let shown = windows::show_pill(app, focus).is_some();
        #[cfg(target_os = "linux")]
        if shown && let Some(k) = self.kwin().await {
            // A freshly shown window is mapped a moment later; try again briefly.
            for _ in 0..10 {
                match k.place_bottom_center(windows::PILL_TITLE, windows::PILL_BOTTOM as i32, focus).await {
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

    /// KDE: remembers where the meeting window is, before it hides.
    #[cfg(target_os = "linux")]
    pub async fn remember_meeting_position(&self, app: &AppHandle) {
        let Some(k) = self.kwin().await else { return };
        match k.position(windows::MEETING_TITLE).await {
            Ok(Some(p)) => {
                use tauri::Manager;
                app.state::<crate::state::AppState>().config.lock().expect("config lock").meetings.kwin_position =
                    Some(p);
            }
            Ok(None) => {}
            Err(e) => log::warn!("reading the meeting window position: {e}"),
        }
    }

    /// KDE: puts the meeting window back where it was, once it is mapped.
    #[cfg(target_os = "linux")]
    pub async fn restore_meeting_position(&self, (x, y): (i32, i32)) {
        let Some(k) = self.kwin().await else { return };
        // A freshly shown window is mapped a moment later; try again briefly.
        for _ in 0..25 {
            match k.move_to(windows::MEETING_TITLE, x, y).await {
                Ok(true) => break,
                Ok(false) => tokio_sleep(Duration::from_millis(20)).await,
                Err(e) => {
                    log::warn!("placing the meeting window: {e}");
                    break;
                }
            }
        }
    }

    /// Gives the keyboard back to the target window.
    pub async fn return_to(&self, target: &Target) {
        #[cfg(target_os = "linux")]
        if let (Some(w), Some(k)) = (&target.window, self.kwin().await) {
            match k.activate(&w.id).await {
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
    /// Returns the restore token to keep.
    pub async fn setup_keyboard(&self, token: Option<String>) -> Result<Option<String>, String> {
        #[cfg(target_os = "linux")]
        {
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
