//! KWin scripting over D-Bus (KDE Plasma 6, SPEC 10.2): which window is active (to paste back
//! into it and to spot terminals), activating it again, and placing the dictation pill at the
//! bottom centre of the active screen, which Wayland clients cannot do themselves.
//!
//! Each call loads a one-shot script into KWin, runs it and unloads it. Scripts answer by calling
//! back into this process over D-Bus (`callDBus` to our unique bus name). Window captions are
//! never sent back or logged; only internal ids and resource classes (app ids) are.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::DesktopError;

const OBJECT_PATH: &str = "/com/engfors/tyst/KWin";
const INTERFACE: &str = "com.engfors.tyst.KWinReply";
const TIMEOUT: Duration = Duration::from_secs(2);
const SEP: char = '\u{1f}';

impl From<zbus::Error> for DesktopError {
    fn from(e: zbus::Error) -> Self {
        DesktopError::KWin(e.to_string())
    }
}

impl From<zbus::fdo::Error> for DesktopError {
    fn from(e: zbus::fdo::Error) -> Self {
        DesktopError::KWin(e.to_string())
    }
}

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<String>>>>>;

struct Replies {
    pending: Pending,
}

#[zbus::interface(name = "com.engfors.tyst.KWinReply")]
impl Replies {
    /// Called by our scripts: `token` names the request, `values` is its answer joined by
    /// U+001F (KWin would send a JavaScript array as `av`, plain strings marshal simply).
    fn reply(&self, token: String, values: String) {
        if let Some(tx) = self.pending.lock().expect("pending lock").remove(&token) {
            let v = if values.is_empty() { Vec::new() } else { values.split(SEP).map(String::from).collect() };
            let _ = tx.send(v);
        }
    }
}

/// A window, as KWin knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRef {
    /// KWin's internal id (a UUID), stable while the window exists.
    pub id: String,
    /// Resource class: the Wayland app id or the X11 WM_CLASS, e.g. `org.kde.konsole`.
    pub class: String,
}

pub struct KWin {
    conn: zbus::Connection,
    pending: Pending,
    next: AtomicU64,
    dir: std::path::PathBuf,
}

/// True on a KDE Plasma session.
pub fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
}

/// Quotes a string for JavaScript source.
fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

impl KWin {
    pub async fn connect() -> Result<Self, DesktopError> {
        let pending: Pending = Arc::default();
        let conn = zbus::connection::Builder::session()?
            .serve_at(OBJECT_PATH, Replies { pending: pending.clone() })?
            .build()
            .await?;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        Ok(Self { conn, pending, next: AtomicU64::new(1), dir })
    }

    /// The active window, if any.
    pub async fn active_window(&self) -> Result<Option<WindowRef>, DesktopError> {
        let v = self
            .run(
                "const w = workspace.activeWindow;\n\
                 reply(w ? [w.internalId.toString(), String(w.resourceClass)] : []);",
            )
            .await?;
        Ok(match v.as_slice() {
            [id, class] => Some(WindowRef { id: id.clone(), class: class.clone() }),
            _ => None,
        })
    }

    /// Activates the window with this internal id. Returns false if it is gone.
    pub async fn activate(&self, id: &str) -> Result<bool, DesktopError> {
        let v = self
            .run(&format!(
                "const id = {};\n\
                 const w = workspace.windowList().find(w => w.internalId.toString() === id);\n\
                 if (w) {{ workspace.activeWindow = w; }}\n\
                 reply([w ? 'ok' : 'gone']);",
                js(id)
            ))
            .await?;
        Ok(v.first().is_some_and(|s| s == "ok"))
    }

    /// Moves the window titled `title` to the bottom centre of the active screen's work area,
    /// `bottom` logical pixels above its lower edge (above the panel), and optionally gives it
    /// keyboard focus. Returns false if no such window is mapped.
    pub async fn place_bottom_center(&self, title: &str, bottom: i32, focus: bool) -> Result<bool, DesktopError> {
        let v = self
            .run(&format!(
                "const w = workspace.windowList().find(w => w.caption === {title});\n\
                 if (w) {{\n\
                   const a = workspace.clientArea(KWin.PlacementArea, workspace.activeScreen, workspace.currentDesktop);\n\
                   const g = w.frameGeometry;\n\
                   w.frameGeometry = {{ x: Math.round(a.x + (a.width - g.width) / 2), y: Math.round(a.y + a.height - g.height - {bottom}), width: g.width, height: g.height }};\n\
                   if ({focus}) {{ workspace.activeWindow = w; }}\n\
                 }}\n\
                 reply([w ? 'ok' : 'gone']);",
                title = js(title),
            ))
            .await?;
        Ok(v.first().is_some_and(|s| s == "ok"))
    }

    /// Runs `body` as a KWin script; it answers by calling `reply([...strings])`.
    async fn run(&self, body: &str) -> Result<Vec<String>, DesktopError> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let token = format!("{}-{n}", std::process::id());
        let me = self.conn.unique_name().ok_or_else(|| DesktopError::KWin("no bus name".into()))?.to_string();
        let source = format!(
            "function reply(values) {{ callDBus({me}, {path}, {iface}, 'Reply', {token}, values.map(String).join('\\u001f')); }}\n{body}\n",
            me = js(&me),
            path = js(OBJECT_PATH),
            iface = js(INTERFACE),
            token = js(&token),
        );
        let file = self.dir.join(format!("tyst-kwin-{token}.js"));
        std::fs::write(&file, source).map_err(|e| DesktopError::KWin(format!("{}: {e}", file.display())))?;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(token.clone(), tx);
        let plugin = format!("tyst-{token}");
        let result = self.load_and_run(&file, &plugin, rx).await;
        self.pending.lock().expect("pending lock").remove(&token);
        let _ = std::fs::remove_file(&file);
        result
    }

    async fn load_and_run(
        &self,
        file: &std::path::Path,
        plugin: &str,
        rx: oneshot::Receiver<Vec<String>>,
    ) -> Result<Vec<String>, DesktopError> {
        let scripting = zbus::Proxy::new(&self.conn, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").await?;
        let id: i32 = scripting.call("loadScript", &(file.display().to_string(), plugin)).await?;
        if id < 0 {
            return Err(DesktopError::KWin("KWin refused the script".into()));
        }
        let ran = async {
            let script =
                zbus::Proxy::new(&self.conn, "org.kde.KWin", format!("/Scripting/Script{id}"), "org.kde.kwin.Script")
                    .await?;
            script.call::<_, _, ()>("run", &()).await?;
            Ok::<_, DesktopError>(())
        }
        .await;
        let answer = match ran {
            Ok(()) => tokio::time::timeout(TIMEOUT, rx)
                .await
                .map_err(|_| DesktopError::KWin("no answer from the KWin script".into()))?
                .map_err(|_| DesktopError::KWin("KWin script answer dropped".into())),
            Err(e) => Err(e),
        };
        let _: Result<bool, _> = scripting.call("unloadScript", &(plugin,)).await;
        answer
    }
}
