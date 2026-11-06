//! XDG desktop portals (SPEC 10.2): global shortcuts through `GlobalShortcuts` (Tauri's
//! shortcut plugin cannot see keys on Wayland), and the paste keystroke through `RemoteDesktop`
//! (one-time consent on KDE, kept with a restore token).
//!
//! Both need a tokio runtime (the app's). A host (non-Flatpak) app first registers its app id,
//! which the desktop uses to remember the shortcuts and the consent.

use std::time::Instant;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts as ShortcutsPortal, NewShortcut};
use ashpd::desktop::remote_desktop::{
    DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::{PersistMode, Session};
use futures_util::StreamExt;

use crate::DesktopError;

impl From<ashpd::Error> for DesktopError {
    fn from(e: ashpd::Error) -> Self {
        if portal_missing(&e) {
            DesktopError::PortalMissing(e.to_string())
        } else {
            DesktopError::Portal(e.to_string())
        }
    }
}

/// True when the error says no portal answered: no session bus, no portal service, or no such
/// interface. A refusal (the user said no) or any error from a portal that did answer is not.
fn portal_missing(e: &ashpd::Error) -> bool {
    match e {
        ashpd::Error::PortalNotFound(_) | ashpd::Error::RequiresVersion(..) => true,
        ashpd::Error::Zbus(z) | ashpd::Error::Portal(ashpd::PortalError::ZBus(z)) => zbus_missing(z),
        _ => false,
    }
}

fn zbus_missing(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::Address(_) | zbus::Error::InterfaceNotFound => true,
        zbus::Error::MethodError(name, ..) => is_missing_name(name.as_str()),
        zbus::Error::FDO(f) => matches!(
            **f,
            zbus::fdo::Error::ServiceUnknown(_)
                | zbus::fdo::Error::UnknownInterface(_)
                | zbus::fdo::Error::UnknownMethod(_)
                | zbus::fdo::Error::UnknownObject(_)
                | zbus::fdo::Error::NameHasNoOwner(_)
        ),
        _ => false,
    }
}

fn is_missing_name(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
    )
}

/// Registers this process with the portal as `app_id` (which needs a matching
/// `<app_id>.desktop` file). Call once, before any other portal use.
pub async fn register_app(app_id: &str) -> Result<(), DesktopError> {
    let id = ashpd::AppID::try_from(app_id).map_err(|e| DesktopError::Portal(e.to_string()))?;
    ashpd::register_host_app(id).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub id: String,
    pub description: String,
    /// In the XDG shortcuts format, e.g. `CTRL+aring` for Ctrl+Å. The desktop may pick another.
    pub preferred: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundShortcut {
    pub id: String,
    /// What the desktop shows for it, e.g. "Ctrl+Å"; empty when the user left it unassigned.
    pub trigger: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutEvent {
    Pressed {
        id: String,
        at: Instant,
    },
    Released {
        id: String,
        at: Instant,
    },
    /// The user changed the bindings in the desktop's settings.
    Changed(Vec<BoundShortcut>),
}

/// Binds the shortcuts (KDE asks the user to confirm them the first time) and reports presses
/// and releases until `stop` fires (then the session closes and this returns `Ok`) or the
/// session ends or fails. `on_bound` gets the bindings once they are set.
pub async fn run_shortcuts(
    specs: &[ShortcutSpec],
    on_bound: impl FnOnce(Vec<BoundShortcut>),
    mut on_event: impl FnMut(ShortcutEvent),
    stop: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), DesktopError> {
    let portal = ShortcutsPortal::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let shortcuts: Vec<NewShortcut> = specs
        .iter()
        .map(|s| NewShortcut::new(s.id.clone(), s.description.clone()).preferred_trigger(s.preferred.as_deref()))
        .collect();
    let bound = portal.bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default()).await?.response()?;
    on_bound(
        bound
            .shortcuts()
            .iter()
            .map(|s| BoundShortcut { id: s.id().into(), trigger: s.trigger_description().into() })
            .collect(),
    );

    let activated = portal.receive_activated().await?;
    let deactivated = portal.receive_deactivated().await?;
    let changed = portal.receive_shortcuts_changed().await?;
    let mut events = futures_util::stream::select(
        futures_util::stream::select(
            activated.map(|a| ShortcutEvent::Pressed { id: a.shortcut_id().into(), at: Instant::now() }),
            deactivated.map(|d| ShortcutEvent::Released { id: d.shortcut_id().into(), at: Instant::now() }),
        ),
        changed.map(|c| {
            ShortcutEvent::Changed(
                c.shortcuts()
                    .iter()
                    .map(|s| BoundShortcut { id: s.id().into(), trigger: s.trigger_description().into() })
                    .collect(),
            )
        }),
    );
    let mut stop = stop;
    loop {
        tokio::select! {
            e = events.next() => match e {
                Some(e) => on_event(e),
                None => break,
            },
            _ = &mut stop => {
                let _ = session.close().await;
                return Ok(());
            }
        }
    }
    let _ = session.close().await;
    Err(DesktopError::Portal("the shortcut session ended".into()))
}

// XKB keysyms.
const XK_CONTROL_L: i32 = 0xffe3;
const XK_SHIFT_L: i32 = 0xffe1;
const XK_V: i32 = 0x0076;

/// Keyboard input through the RemoteDesktop portal, for the paste keystroke.
pub struct PortalKeyboard {
    portal: RemoteDesktop,
    session: Option<Session<RemoteDesktop>>,
    restore_token: Option<String>,
}

impl PortalKeyboard {
    /// `restore_token` from an earlier session skips the consent dialog.
    pub async fn new(restore_token: Option<String>) -> Result<Self, DesktopError> {
        Ok(Self { portal: RemoteDesktop::new().await?, session: None, restore_token })
    }

    /// The token to keep for next time (changes with every session).
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Starts a session (asking for consent unless the restore token still works).
    pub async fn open(&mut self) -> Result<(), DesktopError> {
        if self.session.is_some() {
            return Ok(());
        }
        let t0 = Instant::now();
        let session = self.portal.create_session(Default::default()).await?;
        let opts = SelectDevicesOptions::default()
            .set_devices(ashpd::enumflags2::BitFlags::from(DeviceType::Keyboard))
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(self.restore_token.as_deref());
        self.portal.select_devices(&session, opts).await?.response()?;
        let started = self.portal.start(&session, None, StartOptions::default()).await?.response()?;
        if !started.devices().contains(DeviceType::Keyboard) {
            let _ = session.close().await;
            return Err(DesktopError::Portal("keyboard access was not granted".into()));
        }
        if let Some(t) = started.restore_token() {
            self.restore_token = Some(t.to_string());
        }
        log::info!("keyboard portal session open ({} ms)", t0.elapsed().as_millis());
        self.session = Some(session);
        Ok(())
    }

    pub async fn close(&mut self) {
        if let Some(s) = self.session.take() {
            let _ = s.close().await;
        }
    }

    /// Sends Ctrl+V, or Ctrl+Shift+V for terminals.
    pub async fn paste(&mut self, shift: bool) -> Result<(), DesktopError> {
        self.open().await?;
        let mut keys = vec![XK_CONTROL_L];
        if shift {
            keys.push(XK_SHIFT_L);
        }
        keys.push(XK_V);
        let result = self.press_all(&keys).await;
        if result.is_err() {
            // A session the desktop closed (or revoked): start over on the next paste.
            self.close().await;
        }
        result
    }

    async fn press_all(&self, keys: &[i32]) -> Result<(), DesktopError> {
        let session = self.session.as_ref().expect("opened above");
        for &k in keys {
            self.portal
                .notify_keyboard_keysym(session, k, KeyState::Pressed, NotifyKeyboardKeysymOptions::default())
                .await?;
        }
        for &k in keys.iter().rev() {
            self.portal
                .notify_keyboard_keysym(session, k, KeyState::Released, NotifyKeyboardKeysymOptions::default())
                .await?;
        }
        Ok(())
    }
}

/// Fallback only when the portal is missing ([`DesktopError::PortalMissing`]), never after the
/// user refused the portal's dialog: Ctrl+V (Ctrl+Shift+V) through `ydotool`, which needs
/// `ydotoold` running and access to `/dev/uinput` (SPEC 10.2, set up by the user).
pub fn ydotool_paste(shift: bool) -> Result<(), DesktopError> {
    // Linux input event codes: KEY_LEFTCTRL 29, KEY_LEFTSHIFT 42, KEY_V 47.
    let keys: &[&str] =
        if shift { &["29:1", "42:1", "47:1", "47:0", "42:0", "29:0"] } else { &["29:1", "47:1", "47:0", "29:0"] };
    let status = crate::appimage::host_command("ydotool")
        .arg("key")
        .args(keys)
        .status()
        .map_err(|e| DesktopError::Portal(format!("ydotool: {e}")))?;
    if status.success() { Ok(()) } else { Err(DesktopError::Portal(format!("ydotool exited with {status}"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_absent_portal_counts_as_missing() {
        let missing = ashpd::Error::PortalNotFound("org.freedesktop.portal.RemoteDesktop".try_into().unwrap());
        assert!(matches!(DesktopError::from(missing), DesktopError::PortalMissing(_)));
        let no_service = ashpd::Error::Zbus(zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown("x".into()))));
        assert!(matches!(DesktopError::from(no_service), DesktopError::PortalMissing(_)));
        // The user pressed "Don't allow" in the consent dialog.
        let denied = ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled);
        assert!(matches!(DesktopError::from(denied), DesktopError::Portal(_)));
        let other = ashpd::Error::Response(ashpd::desktop::ResponseError::Other);
        assert!(matches!(DesktopError::from(other), DesktopError::Portal(_)));
    }
}
