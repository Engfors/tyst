//! Dictation (SPEC 8.4): the shortcut gesture, the session, the pill and the paste.
//!
//! One controller thread owns the state and handles commands in order (shortcut presses and
//! releases, the pill's keys, the tray), so a quick press-release-press can never race. Slow
//! steps (models loading, the final decode, the paste) run on that thread too: commands that
//! arrive meanwhile wait their turn, carrying the time they happened for the gesture (a release
//! during a slow start still ends a push-to-talk).
//!
//! Flow: shortcut -> remember the active window -> microphone + pipeline -> pill with live text
//! -> stop -> final text -> preview (Enter pastes, Ctrl+C copies, Esc discards, Tab re-decodes
//! in another language) or direct paste. Esc while listening cancels. A meeting that is
//! recording leaves the dictated speech out of its Me channel (SPEC 15 q4).
//!
//! The pill gets text; logs get phases, timings and lengths only.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tyst_core::router::LanguageMode;
use tyst_core::transcript::Lang;
use tyst_runtime::dictation::{Dictated, Dictation, DictationEvent, DictationOptions};

use crate::config::{PasteMode, Trigger};
use crate::desktop::{Desktop, Pasted, Target};
use crate::state::{AppState, lang_code};
use crate::{sources, tray, windows};

pub const PILL_EVENT: &str = "tyst://dictation";
/// A press held at least this long is push-to-talk (SPEC 8.4).
pub const HOLD: Duration = Duration::from_millis(400);
/// How long "Pasted" or a message stays before the pill hides.
const FLASH: Duration = Duration::from_millis(1800);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    /// Loading models or opening the microphone.
    Starting,
    Listening,
    /// Stopped; decoding the rest.
    Finishing,
    /// The text waits in the pill (preview mode).
    Preview,
    /// Decoding the recording again in another language.
    Redecoding,
    Pasting,
    /// "Pasted" (direct mode), with a Copy button.
    Pasted,
    /// A short note ("Nothing heard", an error), then the pill hides.
    Message,
}

impl Phase {
    /// The microphone is (about to be) on.
    pub fn recording(self) -> bool {
        matches!(self, Phase::Starting | Phase::Listening)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PillState {
    pub phase: Phase,
    /// Final text so far (listening) or the whole text (preview).
    pub text: String,
    /// The open segment's latest guess (listening).
    pub partial: String,
    pub language: &'static str,
    pub paste_mode: PasteMode,
    pub message: Option<String>,
    /// The text will be pasted with Ctrl+Shift+V (a terminal).
    pub terminal: bool,
    pub platform: &'static str,
    /// Languages Tab cycles through (English only with its model installed).
    pub languages: Vec<&'static str>,
    /// The dictation shortcut as the desktop shows it ("Ctrl+Å"), for the preview's hint.
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PillEvent {
    State(PillState),
    Text { text: String, partial: String },
    Level { rms: f32 },
}

pub enum Cmd {
    /// The dictation shortcut went down / up (portal or Tauri shortcut).
    Press(Instant),
    Release(Instant),
    /// Start, or stop when running (tray, `tyst --dictate`).
    Toggle,
    /// Enter while listening.
    Stop,
    /// Esc while listening.
    Cancel,
    /// Enter in the preview, with the (possibly edited) text; `None` uses the text as decoded.
    Paste(Option<String>),
    /// The preview text as the user edited it.
    Edit(String),
    /// Ctrl+C in the preview, or Copy after a direct paste.
    Copy(String),
    /// Esc in the preview; closing the pill.
    Discard,
    /// Tab: Auto -> SV -> EN.
    CycleLanguage,
    /// Hide a flash ("Pasted", a message) if it is still the one shown.
    HideFlash(u64),
    Event(DictationEvent),
}

/// What a shortcut press or release means, given the trigger setting (SPEC 8.4: tap toggles,
/// holding > 400 ms is push-to-talk; a setting can keep only one of the two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    /// Record more and add it to the preview text.
    Resume,
    None,
}

#[derive(Debug, Default)]
pub struct Gesture {
    down_at: Option<Instant>,
    /// This press started the dictation, so its release may end it (push-to-talk).
    started: bool,
}

impl Gesture {
    pub fn press(&mut self, trigger: Trigger, phase: Phase, at: Instant) -> Action {
        if self.down_at.is_some() {
            return Action::None; // key repeat
        }
        self.down_at = Some(at);
        self.started = false;
        match phase {
            Phase::Idle | Phase::Message | Phase::Pasted => {
                self.started = true;
                Action::Start
            }
            Phase::Starting | Phase::Listening if trigger != Trigger::Hold => Action::Stop,
            Phase::Preview => {
                self.started = true;
                Action::Resume
            }
            _ => Action::None,
        }
    }

    pub fn release(&mut self, trigger: Trigger, phase: Phase, at: Instant) -> Action {
        let Some(down) = self.down_at.take() else { return Action::None };
        if !std::mem::take(&mut self.started) || !phase.recording() {
            return Action::None;
        }
        let held = at.saturating_duration_since(down) >= HOLD;
        match trigger {
            Trigger::Hold => Action::Stop,
            Trigger::Hybrid if held => Action::Stop,
            _ => Action::None,
        }
    }
}

struct Ctl {
    app: AppHandle,
    phase: Phase,
    gesture: Gesture,
    session: Option<Dictation>,
    target: Target,
    result: Option<Dictated>,
    /// While recording more for a preview: its (possibly edited) text and recording, which the
    /// new part is added to.
    earlier: Option<(String, Dictated)>,
    text: String,
    partial: String,
    mode: LanguageMode,
    /// Language of the last dictation (short utterances keep it).
    lang: Lang,
    message: Option<String>,
    flash: u64,
    tx: Sender<Cmd>,
}

pub struct DictationHandle {
    tx: Mutex<Sender<Cmd>>,
    phase: Mutex<Phase>,
}

/// Sends a command to the controller.
pub fn send(app: &AppHandle, cmd: Cmd) {
    if let Some(h) = app.try_state::<DictationHandle>() {
        let _ = h.tx.lock().expect("dictation tx lock").send(cmd);
    }
}

/// The current phase, for the tray.
pub fn phase(app: &AppHandle) -> Phase {
    app.try_state::<DictationHandle>().map(|h| *h.phase.lock().expect("phase lock")).unwrap_or(Phase::Idle)
}

/// Starts the controller thread.
pub fn init(app: &AppHandle) {
    let (tx, rx) = mpsc::channel();
    app.manage(DictationHandle { tx: Mutex::new(tx.clone()), phase: Mutex::new(Phase::Idle) });
    let cfg = app.state::<AppState>().config();
    let ctl = Ctl {
        app: app.clone(),
        phase: Phase::Idle,
        gesture: Gesture::default(),
        session: None,
        target: Target::default(),
        result: None,
        earlier: None,
        text: String::new(),
        partial: String::new(),
        mode: LanguageMode::parse(&cfg.dictation.language).unwrap_or_default(),
        lang: cfg.dictation.last_lang,
        message: None,
        flash: 0,
        tx,
    };
    app.manage(PillSnapshot(Mutex::new(ctl.state())));
    std::thread::Builder::new()
        .name("tyst-dictation".into())
        .spawn(move || ctl.run(rx))
        .expect("spawning the dictation thread");
}

/// The pill's view of things, for a pill that (re)loads.
pub fn pill_state(app: &AppHandle) -> Option<PillState> {
    app.try_state::<PillSnapshot>().map(|s| s.0.lock().expect("snapshot lock").clone())
}

/// The last state sent to the pill.
struct PillSnapshot(Mutex<PillState>);

impl Ctl {
    fn run(mut self, rx: Receiver<Cmd>) {
        for cmd in rx {
            self.handle(cmd);
        }
    }

    fn cfg(&self) -> crate::config::DictationSettings {
        self.app.state::<AppState>().config().dictation
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Press(at) => match self.gesture.press(self.cfg().trigger, self.phase, at) {
                Action::Start => self.start(),
                Action::Stop => self.stop(),
                Action::Resume => self.resume(),
                Action::None => {}
            },
            Cmd::Release(at) => {
                if self.gesture.release(self.cfg().trigger, self.phase, at) == Action::Stop {
                    self.stop();
                }
            }
            Cmd::Toggle => match self.phase {
                Phase::Idle | Phase::Message | Phase::Pasted => self.start(),
                Phase::Starting | Phase::Listening => self.stop(),
                Phase::Preview => self.resume(),
                _ => {}
            },
            Cmd::Stop if self.phase.recording() => self.stop(),
            Cmd::Cancel if self.phase.recording() => self.cancel(),
            Cmd::Cancel | Cmd::Discard if matches!(self.phase, Phase::Preview | Phase::Message | Phase::Pasted) => {
                self.close(true)
            }
            Cmd::Discard if self.phase.recording() => self.cancel(),
            Cmd::Paste(text) if self.phase == Phase::Preview => self.paste(text),
            Cmd::Edit(text) if self.phase == Phase::Preview => self.text = text,
            Cmd::Copy(text) if matches!(self.phase, Phase::Preview | Phase::Pasted) => self.copy(&text),
            Cmd::CycleLanguage => self.cycle_language(),
            Cmd::HideFlash(n) if n == self.flash && matches!(self.phase, Phase::Pasted | Phase::Message) => {
                self.close(false)
            }
            Cmd::Event(e) => self.on_event(e),
            _ => {}
        }
    }

    fn languages(&self) -> Vec<&'static str> {
        let dir = self.app.state::<AppState>().config().models_dir();
        if tyst_runtime::fetch::installed(&[tyst_core::models::PARAKEET], &dir) {
            vec!["auto", "sv", "en"]
        } else {
            vec!["auto", "sv"]
        }
    }

    fn state(&self) -> PillState {
        let cfg = self.cfg();
        PillState {
            phase: self.phase,
            text: self.text.clone(),
            partial: self.partial.clone(),
            language: lang_code(self.mode),
            paste_mode: cfg.paste_mode,
            message: self.message.clone(),
            terminal: self.target.class().is_some_and(|c| crate::config::is_terminal(c, &cfg.terminal_classes)),
            platform: std::env::consts::OS,
            languages: self.languages(),
            shortcut: crate::shortcuts::status(&self.app)
                .bound
                .into_iter()
                .find(|(id, trigger)| id == crate::shortcuts::DICTATE && !trigger.is_empty())
                .map(|(_, trigger)| trigger),
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        *self.app.state::<DictationHandle>().phase.lock().expect("phase lock") = phase;
        let st = self.state();
        if let Some(s) = self.app.try_state::<PillSnapshot>() {
            *s.0.lock().expect("snapshot lock") = st.clone();
        }
        let _ = self.app.emit_to(windows::PILL, PILL_EVENT, PillEvent::State(st));
        tray::refresh(&self.app);
    }

    fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    fn start(&mut self) {
        let t0 = Instant::now();
        let app = self.app.clone();
        let desktop = app.state::<Desktop>();
        self.text.clear();
        self.partial.clear();
        self.message = None;
        self.result = None;
        self.earlier = None;
        self.mode = LanguageMode::parse(&self.cfg().language).unwrap_or_default();
        // Where to paste: the window active now, before the pill exists.
        self.target = self.block_on(desktop.target());
        self.set_phase(Phase::Starting);
        self.block_on(desktop.show_pill(&app, true));
        self.listen(t0);
    }

    /// The shortcut in the preview: record more and add it to the text. The paste target, the
    /// language and the pill stay as they are.
    fn resume(&mut self) {
        let t0 = Instant::now();
        let Some(result) = self.result.take() else { return };
        self.earlier = Some((self.text.trim().to_string(), result));
        self.partial.clear();
        self.message = None;
        self.set_phase(Phase::Starting);
        self.listen(t0);
    }

    fn listen(&mut self, t0: Instant) {
        let app = self.app.clone();

        let st = app.state::<AppState>();
        let pipeline = match st.dictation_pipeline(self.mode, self.lang) {
            Ok(p) => p,
            Err(e) => return self.fail(&format!("Could not load the speech model: {e}")),
        };
        let Some(mic) = sources::microphone() else {
            return self.fail("This build has no microphone support.");
        };
        let (tx, rx) = mpsc::channel();
        let opts = DictationOptions { mode: self.mode, initial: self.lang };
        st.set_meeting_dictating(true);
        match Dictation::start(pipeline, mic, opts, tx) {
            Ok(d) => self.session = Some(d),
            Err(e) => {
                st.set_meeting_dictating(false);
                return self.fail(&format!("Microphone: {e}"));
            }
        }
        let fwd = self.tx.clone();
        std::thread::spawn(move || {
            for e in rx {
                if fwd.send(Cmd::Event(e)).is_err() {
                    return;
                }
            }
        });
        log::info!("dictation listening ({} ms after the shortcut)", t0.elapsed().as_millis());
        self.set_phase(Phase::Listening);
    }

    fn on_event(&mut self, e: DictationEvent) {
        match e {
            DictationEvent::Text { committed, partial } if self.phase == Phase::Listening => {
                self.text = self.with_earlier(&committed);
                self.partial = partial;
                let _ = self.app.emit_to(
                    windows::PILL,
                    PILL_EVENT,
                    PillEvent::Text { text: self.text.clone(), partial: self.partial.clone() },
                );
            }
            DictationEvent::Level { rms } => {
                let _ = self.app.emit_to(windows::PILL, PILL_EVENT, PillEvent::Level { rms });
            }
            DictationEvent::CaptureError { message } | DictationEvent::Error { message } if !message.is_empty() => {
                log::warn!("dictation: {message}");
            }
            _ => {}
        }
    }

    fn stop(&mut self) {
        let Some(session) = self.session.take() else { return };
        self.set_phase(Phase::Finishing);
        let result = session.finish();
        self.app.state::<AppState>().set_meeting_dictating(false);
        let mut out = match result {
            Ok(o) => o,
            Err(e) => return self.fail(&format!("Dictation failed: {e}")),
        };
        self.partial.clear();
        if !out.text.is_empty() {
            self.lang = out.lang;
            self.remember_lang();
        }
        self.text = self.with_earlier(&out.text);
        if let Some((_, mut before)) = self.earlier.take() {
            // One recording for Tab to decode again.
            before.audio.extend_from_slice(&out.audio);
            out.audio = before.audio;
        }
        let empty = self.text.is_empty();
        self.result = Some(out);
        if empty {
            self.message = Some("Nothing heard".into());
            self.set_phase(Phase::Message);
            self.flash_timer();
            return;
        }
        match self.cfg().paste_mode {
            PasteMode::Preview => self.set_phase(Phase::Preview),
            PasteMode::Direct => self.paste(None),
        }
    }

    fn cancel(&mut self) {
        if let Some(s) = self.session.take() {
            s.cancel();
        }
        self.app.state::<AppState>().set_meeting_dictating(false);
        // Esc while recording more drops only the new part.
        if let Some((text, result)) = self.earlier.take() {
            self.text = text;
            self.partial.clear();
            self.result = Some(result);
            return self.set_phase(Phase::Preview);
        }
        self.close(true);
    }

    /// `new` after the preview text being added to, if recording more.
    fn with_earlier(&self, new: &str) -> String {
        let before = self.earlier.as_ref().map_or("", |(t, _)| t.as_str());
        [before, new.trim()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join(" ")
    }

    /// Hides the pill and, with `refocus`, gives the keyboard back to the target window.
    fn close(&mut self, refocus: bool) {
        windows::hide_pill(&self.app);
        if refocus {
            let app = self.app.clone();
            let desktop = app.state::<Desktop>();
            self.block_on(desktop.return_to(&self.target));
        }
        self.text.clear();
        self.partial.clear();
        self.message = None;
        self.result = None;
        self.earlier = None;
        self.set_phase(Phase::Idle);
    }

    fn paste(&mut self, edited: Option<String>) {
        let text = edited.map(|t| t.trim().to_string()).unwrap_or_else(|| self.text.clone());
        if text.is_empty() {
            return self.close(true);
        }
        let direct = self.phase != Phase::Preview;
        self.set_phase(Phase::Pasting);
        windows::hide_pill(&self.app);
        let cfg = self.cfg();
        let app = self.app.clone();
        let desktop = app.state::<Desktop>();
        let t0 = Instant::now();
        let r = self.block_on(desktop.paste(
            &self.target,
            &text,
            cfg.restore_clipboard,
            &cfg.terminal_classes,
            cfg.keyboard_token.clone(),
        ));
        match r {
            Ok((pasted, token)) => {
                log::info!(
                    "dictation pasted ({:?}, {} chars, {} ms)",
                    pasted,
                    text.chars().count(),
                    t0.elapsed().as_millis()
                );
                if token != cfg.keyboard_token {
                    self.app.state::<AppState>().update_config(|c| c.dictation.keyboard_token = token);
                }
                self.text = text;
                if pasted != Pasted::Typed {
                    let key = if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" };
                    self.message = Some(if pasted == Pasted::Denied {
                        format!("Paste permission denied. Copied; press {key} to paste.")
                    } else {
                        format!("Copied. Press {key} to paste.")
                    });
                    self.set_phase(Phase::Message);
                    self.block_on(desktop.show_pill(&app, false));
                    self.flash_timer();
                } else if direct {
                    self.set_phase(Phase::Pasted);
                    self.block_on(desktop.show_pill(&app, false));
                    self.flash_timer();
                } else {
                    self.text.clear();
                    self.result = None;
                    self.set_phase(Phase::Idle);
                }
            }
            Err(e) => self.fail(&format!("Could not paste: {e}")),
        }
    }

    fn copy(&mut self, text: &str) {
        let app = self.app.clone();
        let desktop = app.state::<Desktop>();
        if let Err(e) = desktop.copy(text.trim()) {
            return self.fail(&format!("Could not copy: {e}"));
        }
        self.close(true);
    }

    fn cycle_language(&mut self) {
        let langs = self.languages();
        let i = langs.iter().position(|l| *l == lang_code(self.mode)).unwrap_or(0);
        let next = LanguageMode::parse(langs[(i + 1) % langs.len()]).unwrap_or_default();
        self.mode = next;
        match self.phase {
            Phase::Starting | Phase::Listening => {
                if let Some(s) = &self.session {
                    s.set_language(next);
                }
                self.set_phase(self.phase);
            }
            Phase::Preview => self.redecode(),
            _ => {}
        }
    }

    /// Decodes the kept recording again in the chosen language (Tab in the preview).
    fn redecode(&mut self) {
        let Some(audio) = self.result.as_ref().map(|r| r.audio.clone()) else { return };
        self.set_phase(Phase::Redecoding);
        let t0 = Instant::now();
        let st = self.app.state::<AppState>();
        let lang = if self.mode == LanguageMode::English { Lang::En } else { Lang::Sv };
        let r = st
            .retranscribe_pipeline(self.mode, lang)
            .and_then(|p| tyst_runtime::dictation::retranscribe(p, &audio).map_err(|e| e.to_string()));
        match r {
            Ok((text, lang)) => {
                log::info!("dictation re-decoded in {:?} ({} ms)", self.mode, t0.elapsed().as_millis());
                self.text = text;
                self.lang = lang;
                self.remember_lang();
                self.set_phase(Phase::Preview);
            }
            Err(e) => self.fail(&format!("Could not decode again: {e}")),
        }
    }

    fn remember_lang(&self) {
        let lang = self.lang;
        self.app.state::<AppState>().update_config(|c| c.dictation.last_lang = lang);
    }

    fn fail(&mut self, message: &str) {
        log::error!("dictation: {message}");
        if let Some(s) = self.session.take() {
            s.cancel();
            self.app.state::<AppState>().set_meeting_dictating(false);
        }
        self.message = Some(message.to_string());
        self.set_phase(Phase::Message);
        let app = self.app.clone();
        let desktop = app.state::<Desktop>();
        self.block_on(desktop.show_pill(&app, false));
        self.flash_timer();
    }

    fn flash_timer(&mut self) {
        self.flash += 1;
        let (n, tx) = (self.flash, self.tx.clone());
        let wait = if self.phase == Phase::Message && self.message.as_deref().is_some_and(|m| m.len() > 20) {
            FLASH * 2
        } else {
            FLASH
        };
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            let _ = tx.send(Cmd::HideFlash(n));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *T0.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    #[test]
    fn hybrid_tap_toggles() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Idle, at(0)), Action::Start);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(150)), Action::None);
        assert_eq!(g.press(Trigger::Hybrid, Phase::Listening, at(3000)), Action::Stop);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Finishing, at(3100)), Action::None);
        // In the preview, the shortcut records more; a tap keeps recording.
        assert_eq!(g.press(Trigger::Hybrid, Phase::Preview, at(5000)), Action::Resume);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(5100)), Action::None);
        assert_eq!(g.press(Trigger::Hybrid, Phase::Listening, at(8000)), Action::Stop);
    }

    #[test]
    fn holding_in_the_preview_records_more_until_release() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Preview, at(0)), Action::Resume);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(2000)), Action::Stop);
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hold, Phase::Preview, at(0)), Action::Resume);
        assert_eq!(g.release(Trigger::Hold, Phase::Listening, at(300)), Action::Stop);
    }

    #[test]
    fn hybrid_hold_is_push_to_talk() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Idle, at(0)), Action::Start);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(2500)), Action::Stop);
        // Release while still starting (models loading) stops too.
        assert_eq!(g.press(Trigger::Hybrid, Phase::Idle, at(4000)), Action::Start);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Starting, at(4500)), Action::Stop);
    }

    #[test]
    fn key_repeat_is_ignored() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Idle, at(0)), Action::Start);
        assert_eq!(g.press(Trigger::Hybrid, Phase::Listening, at(30)), Action::None);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(900)), Action::Stop);
    }

    #[test]
    fn toggle_only_ignores_holding() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Toggle, Phase::Idle, at(0)), Action::Start);
        assert_eq!(g.release(Trigger::Toggle, Phase::Listening, at(2000)), Action::None);
        assert_eq!(g.press(Trigger::Toggle, Phase::Listening, at(4000)), Action::Stop);
    }

    #[test]
    fn hold_only_stops_on_any_release() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hold, Phase::Idle, at(0)), Action::Start);
        assert_eq!(g.release(Trigger::Hold, Phase::Listening, at(100)), Action::Stop);
    }

    #[test]
    fn a_flash_does_not_block_the_next_dictation() {
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Pasted, at(0)), Action::Start);
        assert_eq!(g.release(Trigger::Hybrid, Phase::Listening, at(100)), Action::None);
        let mut g = Gesture::default();
        assert_eq!(g.press(Trigger::Hybrid, Phase::Message, at(0)), Action::Start);
    }
}
