//! Secrets in the OS keychain (SPEC 11): the keyboard portal's restore token (Linux, issue #8), and
//! the cleanup of the update check's old GitHub token. Linux uses the Secret Service API (KDE
//! Wallet, GNOME Keyring), macOS the login keychain. Values never go into the config, the logs or
//! the UI; the config only remembers *that* a secret was stored, so nothing asks the keychain
//! (which may ask the user to unlock it) before a secret is needed.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// The GitHub token versions up to 0.1.0 stored for the update check; only ever deleted now.
pub const LEGACY_GITHUB_TOKEN: &str = "github-token";
/// The RemoteDesktop portal's restore token, so paste does not ask for consent again.
pub const KEYBOARD_TOKEN: &str = "keyboard-portal-token";

const SERVICE: &str = "com.engfors.tyst";

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;

    use secret_service::{EncryptionType, SecretService};

    use super::SERVICE;

    fn attrs(key: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", SERVICE), ("key", key)])
    }

    fn err(e: secret_service::Error) -> String {
        format!("keychain (Secret Service): {e}")
    }

    pub async fn get(key: &str) -> Result<Option<String>, String> {
        let ss = SecretService::connect(EncryptionType::Dh).await.map_err(err)?;
        let found = ss.search_items(attrs(key)).await.map_err(err)?;
        let Some(item) = found.unlocked.into_iter().chain(found.locked).next() else { return Ok(None) };
        item.ensure_unlocked().await.map_err(err)?;
        let secret = item.get_secret().await.map_err(err)?;
        String::from_utf8(secret).map(Some).map_err(|_| "keychain: the stored value is not text".into())
    }

    pub async fn set(key: &str, value: Option<&str>) -> Result<(), String> {
        let ss = SecretService::connect(EncryptionType::Dh).await.map_err(err)?;
        match value {
            Some(v) => {
                let collection = match ss.get_default_collection().await {
                    Ok(c) => c,
                    Err(_) => ss.get_any_collection().await.map_err(err)?,
                };
                collection.ensure_unlocked().await.map_err(err)?;
                let label = format!("Tyst: {key}");
                collection.create_item(&label, attrs(key), v.as_bytes(), true, "text/plain").await.map_err(err)?;
            }
            None => {
                let found = ss.search_items(attrs(key)).await.map_err(err)?;
                for item in found.unlocked.iter().chain(&found.locked) {
                    item.delete().await.map_err(err)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

    use super::SERVICE;

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    pub async fn get(key: &str) -> Result<Option<String>, String> {
        match get_generic_password(SERVICE, key) {
            Ok(bytes) => {
                String::from_utf8(bytes).map(Some).map_err(|_| "keychain: the stored value is not text".into())
            }
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(format!("keychain: {e}")),
        }
    }

    pub async fn set(key: &str, value: Option<&str>) -> Result<(), String> {
        match value {
            Some(v) => set_generic_password(SERVICE, key, v.as_bytes()).map_err(|e| format!("keychain: {e}")),
            None => match delete_generic_password(SERVICE, key) {
                Err(e) if e.code() != NOT_FOUND => Err(format!("keychain: {e}")),
                _ => Ok(()),
            },
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    pub async fn get(_key: &str) -> Result<Option<String>, String> {
        Err("no keychain on this platform".into())
    }

    pub async fn set(_key: &str, _value: Option<&str>) -> Result<(), String> {
        Err("no keychain on this platform".into())
    }
}

pub use imp::{get, set};

/// The keyboard restore token, read from the keychain once and kept in memory.
#[derive(Default)]
pub struct KeyboardToken {
    /// `None` until read.
    cached: Mutex<Option<Option<String>>>,
}

impl KeyboardToken {
    pub async fn get(&self, app: &AppHandle) -> Option<String> {
        if let Some(t) = self.cached.lock().expect("token lock").clone() {
            return t;
        }
        let token = if app.state::<AppState>().config().dictation.keyboard_access {
            get(KEYBOARD_TOKEN).await.unwrap_or_else(|e| {
                log::warn!("keyboard token: {e}");
                None
            })
        } else {
            None
        };
        *self.cached.lock().expect("token lock") = Some(token.clone());
        token
    }

    /// Keeps a new token: in memory at once, in the keychain if it can.
    pub async fn set(&self, app: &AppHandle, token: Option<String>) {
        let old = self.cached.lock().expect("token lock").replace(token.clone());
        if old.as_ref() == Some(&token) {
            return;
        }
        let stored = match set(KEYBOARD_TOKEN, token.as_deref()).await {
            Ok(()) => token.is_some(),
            Err(e) => {
                log::warn!("keyboard token not kept in the keychain (paste asks again next start): {e}");
                false
            }
        };
        app.state::<AppState>().update_config(|c| c.dictation.keyboard_access = stored);
    }

    /// Moves a token an older version kept in `config.toml` into the keychain (issue #8).
    pub async fn migrate(&self, app: &AppHandle) {
        let legacy = app.state::<AppState>().config().dictation.keyboard_token;
        let Some(token) = legacy else { return };
        log::info!("moving the keyboard token from the config into the keychain");
        self.set(app, Some(token)).await;
        app.state::<AppState>().update_config(|c| c.dictation.keyboard_token = None);
    }
}
