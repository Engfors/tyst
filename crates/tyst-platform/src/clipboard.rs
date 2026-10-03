//! The system clipboard for dictation paste (SPEC 8.4): put the text there, paste, then put back
//! what the user had (text only). On Wayland this uses the data-control protocol, so it works
//! without a focused window (SPEC 10.2); on X11 and macOS the usual clipboard.
//!
//! Never logs clipboard contents.

use std::sync::Mutex;

use crate::DesktopError;

pub struct Clipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub fn new() -> Self {
        Self { inner: Mutex::new(None) }
    }

    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T, DesktopError> {
        let mut guard = self.inner.lock().expect("clipboard lock");
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(|e| DesktopError::Clipboard(e.to_string()))?);
        }
        f(guard.as_mut().expect("set above")).map_err(|e| DesktopError::Clipboard(e.to_string()))
    }

    /// The current text, or `None` when the clipboard is empty or holds something else.
    pub fn text(&self) -> Option<String> {
        self.with(|c| c.get_text()).ok()
    }

    /// Replaces the clipboard with `text`. `transient` keeps it out of clipboard managers'
    /// history (for the moment between paste and restore).
    pub fn set_text(&self, text: &str, transient: bool) -> Result<(), DesktopError> {
        self.with(|c| {
            let set = c.set();
            #[cfg(target_os = "linux")]
            let set = if transient { arboard::SetExtLinux::exclude_from_history(set) } else { set };
            #[cfg(target_os = "macos")]
            let set = if transient { arboard::SetExtApple::exclude_from_history(set) } else { set };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let _ = transient;
            set.text(text.to_owned())
        })
    }
}
