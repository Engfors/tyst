//! Which other apps are using the microphone (SPEC 9.4, opt-in meeting detection). The app polls
//! [`MicWatcher::users`] and asks before it records; nothing here starts a recording.
//!
//! Linux: the PipeWire graph. Every app that captures audio has an input stream node
//! (`media.class = Stream/Input/Audio`); its client carries the app's name and binary.
//! macOS: Core Audio's process objects (`kAudioHardwarePropertyProcessObjectList`), with
//! `kAudioProcessPropertyIsRunningInput` and the bundle id.
//!
//! Tyst's own streams (this process) are left out.

/// An app using the microphone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicUser {
    /// Stable while the app keeps the microphone (PipeWire node id, Core Audio process object).
    pub id: u64,
    /// What the app calls itself (`application.name`; the bundle id on macOS).
    pub name: String,
    /// Executable name (Linux) or bundle id (macOS), for matching against the app list.
    pub binary: String,
}

impl MicUser {
    /// Whether this app matches one of `apps` (case-insensitive, part of the name or binary:
    /// `teams` matches `teams-for-linux` and `com.microsoft.teams2`).
    pub fn matches(&self, apps: &[String]) -> bool {
        let name = self.name.to_lowercase();
        let binary = self.binary.to_lowercase();
        apps.iter()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty())
            .any(|a| name.contains(&a) || binary.contains(&a))
    }

    /// The name to show: "Firefox", "Microsoft Teams", or the binary.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() { &self.binary } else { &self.name }
    }
}

/// The apps the setting starts with: meeting clients, and browsers (a browser using the
/// microphone is probably in Meet, Teams or Zoom on the web).
pub const DEFAULT_APPS: &[&str] = &[
    "teams", "zoom", "webex", "slack", "discord", "skype", "jitsi", "firefox", "chrome", "chromium", "brave",
    "vivaldi", "msedge", "opera", "safari", "zen",
];

#[cfg(all(target_os = "linux", feature = "pipewire"))]
pub use linux::MicWatcher;
#[cfg(all(target_os = "macos", feature = "macos-tap"))]
pub use macos::MicWatcher;

#[cfg(all(target_os = "linux", feature = "pipewire"))]
mod linux {
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use pipewire as pw;
    use pw::types::ObjectType;

    use super::MicUser;
    use crate::CaptureError;

    #[derive(Default)]
    struct Graph {
        /// client id -> (application.name, application.process.binary, pid)
        clients: BTreeMap<u32, (String, String, u32)>,
        /// node id -> (client id, application.name)
        inputs: BTreeMap<u32, (u32, String)>,
    }

    impl Graph {
        fn users(&self, own_pid: u32) -> Vec<MicUser> {
            self.inputs
                .iter()
                .filter_map(|(&id, (client, node_app))| {
                    let (app, binary, pid) = self.clients.get(client).cloned().unwrap_or_default();
                    if pid == own_pid {
                        return None;
                    }
                    let name = if node_app.is_empty() { app } else { node_app.clone() };
                    Some(MicUser { id: id as u64, name, binary })
                })
                .collect()
        }
    }

    /// Watches the PipeWire registry on its own thread.
    pub struct MicWatcher {
        graph: Arc<Mutex<Graph>>,
        quit: pw::channel::Sender<()>,
        thread: Option<JoinHandle<()>>,
    }

    impl MicWatcher {
        pub fn start() -> Result<Self, CaptureError> {
            let graph = Arc::new(Mutex::new(Graph::default()));
            let (quit_tx, quit_rx) = pw::channel::channel::<()>();
            let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
            let g = graph.clone();
            let thread = std::thread::Builder::new()
                .name("tyst-mic-watch".into())
                .spawn(move || {
                    if let Err(e) = run(g, quit_rx, &ready_tx) {
                        let _ = ready_tx.send(Err(e));
                    }
                })
                .map_err(|e| CaptureError::Device(e.to_string()))?;
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(Self { graph, quit: quit_tx, thread: Some(thread) }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(CaptureError::Device(format!("pipewire: {e}")))
                }
                Err(_) => {
                    let _ = quit_tx.send(());
                    let _ = thread.join();
                    Err(CaptureError::Device("pipewire: no answer from the PipeWire daemon".into()))
                }
            }
        }

        /// Apps with a capture stream open right now.
        pub fn users(&self) -> Vec<MicUser> {
            self.graph.lock().expect("graph lock").users(std::process::id())
        }
    }

    impl Drop for MicWatcher {
        fn drop(&mut self) {
            let _ = self.quit.send(());
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn run(
        graph: Arc<Mutex<Graph>>,
        quit: pw::channel::Receiver<()>,
        ready: &mpsc::SyncSender<Result<(), String>>,
    ) -> Result<(), String> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
        let core = context.connect_rc(None).map_err(|e| e.to_string())?;
        let registry = Rc::new(core.get_registry().map_err(|e| e.to_string())?);
        let added = graph.clone();
        let _listener = registry
            .add_listener_local()
            .global(move |global| {
                let Some(props) = global.props else { return };
                let get = |k: &str| props.get(k).unwrap_or_default().to_string();
                let mut g = added.lock().expect("graph lock");
                match global.type_ {
                    ObjectType::Client => {
                        let pid = get("application.process.id").parse().unwrap_or(0);
                        g.clients.insert(global.id, (get("application.name"), get("application.process.binary"), pid));
                    }
                    ObjectType::Node if props.get("media.class") == Some("Stream/Input/Audio") => {
                        let client = get("client.id").parse().unwrap_or(u32::MAX);
                        g.inputs.insert(global.id, (client, get("application.name")));
                    }
                    _ => {}
                }
            })
            .global_remove(move |id| {
                let mut g = graph.lock().expect("graph lock");
                g.inputs.remove(&id);
                g.clients.remove(&id);
            })
            .register();
        let _quit = quit.attach(mainloop.loop_(), {
            let mainloop = mainloop.clone();
            move |()| mainloop.quit()
        });
        let _ = ready.send(Ok(()));
        mainloop.run();
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn joins_nodes_with_their_clients_and_skips_our_own() {
            let mut g = Graph::default();
            g.clients.insert(10, ("Firefox".into(), "firefox".into(), 4000));
            g.clients.insert(11, ("Tyst".into(), "tyst".into(), 99));
            g.inputs.insert(50, (10, "Firefox".into()));
            g.inputs.insert(51, (11, "Tyst".into()));
            let users = g.users(99);
            assert_eq!(users, vec![MicUser { id: 50, name: "Firefox".into(), binary: "firefox".into() }]);
        }
    }
}

#[cfg(all(target_os = "macos", feature = "macos-tap"))]
mod macos {
    use objc2::rc::Retained;
    use objc2_core_audio::{
        AudioObjectID, kAudioHardwarePropertyProcessObjectList, kAudioObjectSystemObject,
        kAudioProcessPropertyBundleID, kAudioProcessPropertyIsRunningInput, kAudioProcessPropertyPID,
    };
    use objc2_foundation::NSString;

    use super::MicUser;
    use crate::CaptureError;
    use crate::macos_tap::{get, get_array};

    /// Asks Core Audio on every call (cheap: a handful of properties per process).
    pub struct MicWatcher;

    impl MicWatcher {
        pub fn start() -> Result<Self, CaptureError> {
            Ok(Self)
        }

        pub fn users(&self) -> Vec<MicUser> {
            match processes() {
                Ok(u) => u,
                Err(e) => {
                    log::warn!("microphone users: {e}");
                    Vec::new()
                }
            }
        }
    }

    fn processes() -> Result<Vec<MicUser>, CaptureError> {
        let own = std::process::id() as i32;
        let objects: Vec<AudioObjectID> =
            unsafe { get_array(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyProcessObjectList)? };
        let mut users = Vec::new();
        for object in objects {
            unsafe {
                let running: u32 = get(object, kAudioProcessPropertyIsRunningInput, 0).unwrap_or(0);
                if running == 0 {
                    continue;
                }
                let pid: i32 = get(object, kAudioProcessPropertyPID, -1).unwrap_or(-1);
                if pid == own {
                    continue;
                }
                let bundle: *mut NSString = get(object, kAudioProcessPropertyBundleID, std::ptr::null_mut())?;
                let bundle = Retained::from_raw(bundle).map(|s| s.to_string()).unwrap_or_default();
                users.push(MicUser { id: object as u64, name: bundle.clone(), binary: bundle });
            }
        }
        Ok(users)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_meeting_apps_by_part_of_the_name() {
        let apps: Vec<String> = DEFAULT_APPS.iter().map(|s| s.to_string()).collect();
        let user = |name: &str, binary: &str| MicUser { id: 1, name: name.into(), binary: binary.into() };
        assert!(user("teams-for-linux", "teams-for-linux").matches(&apps));
        assert!(user("com.microsoft.teams2", "com.microsoft.teams2").matches(&apps));
        assert!(user("Firefox", "firefox").matches(&apps));
        assert!(user("Google Chrome", "chrome").matches(&apps));
        assert!(user("", "zoom").matches(&apps));
        assert!(!user("OBS", "obs").matches(&apps));
        assert!(!user("Firefox", "firefox").matches(&["  ".to_string()]));
        assert_eq!(user("", "zoom").display_name(), "zoom");
    }
}
