//! App state: settings, the warm models, and the meeting session state machine
//! (idle → recording ⇄ paused → naming → saved, SPEC 8.3).
//!
//! Events for the UI go out as `tyst://meeting` (see [`UiEvent`]). Transcript text only ever
//! travels to the meeting window and the Markdown file; logs carry states, timings and errors.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tyst_core::pipeline::ChannelPipeline;
use tyst_core::router::LanguageMode;
use tyst_core::transcript::{Channel, Lang};
use tyst_core::vad::SileroVad;
use tyst_core::vocabulary::VocabularyRules;
use tyst_runtime::meeting::{Meeting, MeetingEvent, MeetingOptions, StoppedMeeting};
use tyst_runtime::{EngineOptions, Runtime};

use crate::config::{self, Config};
use crate::{sources, tray, windows};

pub const MEETING_EVENT: &str = "tyst://meeting";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    /// Loading models before recording starts.
    Starting,
    Recording,
    Paused,
    /// Stopped, waiting for a title.
    Naming,
}

/// One finished turn line in the meeting window.
#[derive(Debug, Clone, Serialize)]
pub struct UiSegment {
    pub id: u64,
    pub channel: Channel,
    pub lang: Lang,
    pub start_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiPartial {
    pub channel: Channel,
    pub segment_id: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Naming {
    /// What the file is called without a title, shown as the placeholder.
    pub placeholder: String,
    pub path_preview: String,
    pub seconds: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    State(Snapshot),
    Partial(UiPartial),
    Final(UiSegment),
    Dropped { channel: Channel, segment_id: u64 },
    Level { channel: Channel, rms: f32 },
    Warning { message: String },
    Saved { path: String },
}

/// Everything the meeting window needs to (re)draw itself.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub phase: Phase,
    /// Wall time since recording started.
    pub elapsed_ms: u64,
    pub language: &'static str,
    pub labels: tyst_core::transcript::SpeakerLabels,
    pub compact: bool,
    pub segments: Vec<UiSegment>,
    pub partials: Vec<UiPartial>,
    pub naming: Option<Naming>,
    pub warning: Option<String>,
    pub channels: Vec<Channel>,
}

enum Session {
    Idle,
    Starting,
    Recording(Meeting),
    Naming { stopped: StoppedMeeting, token: u64 },
}

#[derive(Default)]
struct Live {
    segments: Vec<UiSegment>,
    partials: Vec<UiPartial>,
    warning: Option<String>,
    started: Option<Instant>,
    channels: Vec<Channel>,
}

pub struct AppState {
    pub config: Mutex<Config>,
    runtime: Mutex<Option<Runtime>>,
    session: Mutex<Session>,
    live: Mutex<Live>,
    naming_token: Mutex<u64>,
    /// When the models were last needed (SPEC 5.2: unload after N idle minutes).
    last_used: Mutex<Instant>,
}

pub fn lang_code(mode: LanguageMode) -> &'static str {
    match mode {
        LanguageMode::Auto => "auto",
        LanguageMode::Swedish => "sv",
        LanguageMode::English => "en",
    }
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config: Mutex::new(config),
            runtime: Mutex::new(None),
            session: Mutex::new(Session::Idle),
            live: Mutex::new(Live::default()),
            naming_token: Mutex::new(0),
            last_used: Mutex::new(Instant::now()),
        }
    }

    pub fn config(&self) -> Config {
        self.config.lock().expect("config lock").clone()
    }

    pub fn phase(&self) -> Phase {
        match &*self.session.lock().expect("session lock") {
            Session::Idle => Phase::Idle,
            Session::Starting => Phase::Starting,
            Session::Recording(m) if m.is_paused() => Phase::Paused,
            Session::Recording(_) => Phase::Recording,
            Session::Naming { .. } => Phase::Naming,
        }
    }

    pub fn language(&self) -> LanguageMode {
        match &*self.session.lock().expect("session lock") {
            Session::Recording(m) => m.language(),
            _ => LanguageMode::parse(&self.config().language).unwrap_or_default(),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let phase = self.phase();
        let cfg = self.config();
        let naming = match &*self.session.lock().expect("session lock") {
            Session::Naming { stopped, .. } => Some(naming_info(stopped, &cfg)),
            _ => None,
        };
        let live = self.live.lock().expect("live lock");
        Snapshot {
            phase,
            elapsed_ms: live.started.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0),
            language: lang_code(self.language()),
            labels: cfg.labels.clone(),
            compact: cfg.meetings.compact,
            segments: live.segments.clone(),
            partials: live.partials.clone(),
            naming,
            warning: live.warning.clone(),
            channels: live.channels.clone(),
        }
    }

    /// Loads the models in the background if they are not loaded yet (keeps them warm).
    pub fn preload(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            let st = app.state::<AppState>();
            if let Err(e) = st.ensure_runtime() {
                log::warn!("model preload failed: {e}");
            }
        });
    }

    fn engine_options(cfg: &Config) -> EngineOptions {
        EngineOptions {
            models_dir: cfg.models_dir.clone(),
            threads: cfg.threads.max(1),
            mode: LanguageMode::parse(&cfg.language).unwrap_or_default(),
            vocabulary: VocabularyRules::new(&config::load_vocabulary()),
        }
    }

    fn ensure_runtime(&self) -> Result<(), String> {
        *self.last_used.lock().expect("last used lock") = Instant::now();
        let mut rt = self.runtime.lock().expect("runtime lock");
        if rt.is_none() {
            let cfg = self.config();
            let opts = Self::engine_options(&cfg);
            let loaded = Runtime::load(&opts).map_err(|e| e.to_string())?;
            log::info!("models ready ({:.1}s)", loaded.load_time.as_secs_f32());
            *rt = Some(loaded);
        }
        Ok(())
    }

    /// Changes settings from the app itself (not the settings form) and saves them.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let mut cfg = self.config.lock().expect("config lock");
        f(&mut cfg);
        if let Err(e) = cfg.save() {
            log::error!("saving config: {e}");
        }
    }

    /// A live pipeline for dictation (loads the models first if needed).
    pub fn dictation_pipeline(&self, mode: LanguageMode, lang: Lang) -> Result<ChannelPipeline<SileroVad>, String> {
        self.ensure_runtime()?;
        let rt = self.runtime.lock().expect("runtime lock");
        rt.as_ref().ok_or("models not loaded")?.pipeline_with(Channel::Me, true, mode, lang).map_err(|e| e.to_string())
    }

    /// A pipeline without partials, for decoding kept dictation audio again.
    pub fn retranscribe_pipeline(&self, mode: LanguageMode, lang: Lang) -> Result<ChannelPipeline<SileroVad>, String> {
        self.ensure_runtime()?;
        let rt = self.runtime.lock().expect("runtime lock");
        rt.as_ref().ok_or("models not loaded")?.pipeline_with(Channel::Me, false, mode, lang).map_err(|e| e.to_string())
    }

    /// Leaves dictated speech out of a recording meeting's Me channel (SPEC 15 q4).
    pub fn set_meeting_dictating(&self, on: bool) {
        if let Session::Recording(m) = &*self.session.lock().expect("session lock")
            && let Err(e) = m.set_dictating(on)
        {
            log::error!("marking dictation in the meeting: {e}");
        }
    }

    /// Drops the loaded models (thread count or models folder changed); reloaded on next use.
    pub fn unload_runtime(&self) {
        *self.runtime.lock().expect("runtime lock") = None;
    }

    /// Unloads the models after `models_idle_minutes` without a meeting or dictation (SPEC 5.2,
    /// 11: next to no memory and CPU while idle). They load again on the next use, which then
    /// takes a second or two longer.
    pub fn unload_when_idle(app: &AppHandle) {
        let app = app.clone();
        std::thread::Builder::new()
            .name("tyst-idle".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(30));
                    let st = app.state::<AppState>();
                    let busy =
                        st.phase() != Phase::Idle || crate::dictation::phase(&app) != crate::dictation::Phase::Idle;
                    let mut last = st.last_used.lock().expect("last used lock");
                    if busy {
                        *last = Instant::now();
                        continue;
                    }
                    let minutes = st.config().models_idle_minutes;
                    if minutes == 0 || last.elapsed() < Duration::from_secs(60 * minutes as u64) {
                        continue;
                    }
                    *last = Instant::now();
                    drop(last);
                    let mut rt = st.runtime.lock().expect("runtime lock");
                    if rt.take().is_some() {
                        log::info!("models unloaded after {minutes} idle minutes");
                    }
                }
            })
            .expect("spawning the idle thread");
    }

    /// New vocabulary rules, used from the next meeting on.
    pub fn set_vocabulary(&self, rules: VocabularyRules) {
        if let Some(rt) = self.runtime.lock().expect("runtime lock").as_mut() {
            rt.vocabulary = Arc::new(rules);
        }
    }

    pub fn emit_state(&self, app: &AppHandle) {
        let _ = app.emit(MEETING_EVENT, UiEvent::State(self.snapshot()));
        tray::refresh(app);
    }
}

fn naming_info(stopped: &StoppedMeeting, cfg: &Config) -> Naming {
    let placeholder = format!("{} Meeting", stopped.session().info.started_at.format("%Y-%m-%d %H%M"));
    Naming {
        placeholder,
        path_preview: stopped.preview_path(None).display().to_string(),
        seconds: cfg.meetings.name_prompt_seconds,
    }
}

/// Starts a meeting: saves a meeting still waiting for its name, loads the models if needed
/// (off the caller's thread), then starts capture on Me (and Others when enabled).
pub fn start_meeting(app: &AppHandle) -> Result<(), String> {
    // A meeting prompt that is still showing has been answered.
    crate::dictation::send(app, crate::dictation::Cmd::PromptAnswer(false));
    let st = app.state::<AppState>();
    if st.phase() == Phase::Naming {
        save_meeting(app, None)?;
    }
    {
        let mut session = st.session.lock().expect("session lock");
        if !matches!(*session, Session::Idle) {
            return Ok(());
        }
        *session = Session::Starting;
    }
    let cfg = st.config();
    let Some(dir) = cfg.transcripts_dir.clone() else {
        *st.session.lock().expect("session lock") = Session::Idle;
        return Err("Choose a transcripts folder in Settings first.".into());
    };
    *st.live.lock().expect("live lock") = Live::default();
    st.emit_state(app);
    if cfg.meetings.show_window_on_start {
        windows::show_meeting(app);
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let result = st.ensure_runtime().and_then(|_| begin(&app, &cfg, dir));
        if let Err(e) = result {
            log::error!("could not start meeting: {e}");
            *st.session.lock().expect("session lock") = Session::Idle;
            st.live.lock().expect("live lock").warning = Some(e.clone());
            st.emit_state(&app);
            let _ = app.emit(MEETING_EVENT, UiEvent::Warning { message: e });
        }
    });
    Ok(())
}

fn begin(app: &AppHandle, cfg: &Config, dir: PathBuf) -> Result<(), String> {
    let st = app.state::<AppState>();
    let mut srcs = Vec::new();
    let mic = sources::microphone().ok_or("this build has no microphone support")?;
    srcs.push((Channel::Me, mic));
    let mut warning = None;
    if cfg.meetings.system_audio {
        match sources::system_audio() {
            Some(s) => srcs.push((Channel::Others, s)),
            None => {
                warning = Some("System audio capture is not available on this platform yet; recording Me only.".into())
            }
        }
    }
    let channels: Vec<Channel> = srcs.iter().map(|(c, _)| *c).collect();
    let (tx, rx) = mpsc::channel();
    let opts = MeetingOptions {
        transcripts_dir: dir,
        labels: cfg.labels.clone(),
        app: format!("Tyst {}", env!("CARGO_PKG_VERSION")),
        mode: LanguageMode::parse(&cfg.language).unwrap_or_default(),
        sources: srcs,
        echo_cancellation: cfg.meetings.echo_cancellation,
    };
    let meeting = {
        let rt = st.runtime.lock().expect("runtime lock");
        let rt = rt.as_ref().ok_or("models not loaded")?;
        Meeting::start(rt, opts, tx).map_err(|e| e.to_string())?
    };
    {
        let mut live = st.live.lock().expect("live lock");
        live.started = Some(Instant::now());
        live.channels = channels;
        live.warning = warning.clone();
    }
    *st.session.lock().expect("session lock") = Session::Recording(meeting);
    if let Some(w) = warning {
        let _ = app.emit(MEETING_EVENT, UiEvent::Warning { message: w });
    }
    st.emit_state(app);
    spawn_forwarder(app.clone(), rx);
    Ok(())
}

/// Forwards pipeline events to the meeting window and keeps the live transcript for redraws.
fn spawn_forwarder(app: AppHandle, rx: mpsc::Receiver<MeetingEvent>) {
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        for e in rx {
            let ui = match e {
                MeetingEvent::Partial { channel, segment_id, text } => {
                    let p = UiPartial { channel, segment_id, text };
                    let mut live = st.live.lock().expect("live lock");
                    live.partials.retain(|x| x.channel != channel);
                    live.partials.push(p.clone());
                    UiEvent::Partial(p)
                }
                MeetingEvent::Final(s) => {
                    let seg = UiSegment {
                        id: s.id,
                        channel: s.channel,
                        lang: s.lang,
                        start_ms: s.start.as_millis() as u64,
                        text: s.text,
                    };
                    let mut live = st.live.lock().expect("live lock");
                    live.partials.retain(|x| !(x.channel == seg.channel && x.segment_id == seg.id));
                    live.segments.push(seg.clone());
                    UiEvent::Final(seg)
                }
                MeetingEvent::Dropped { channel, segment_id } => {
                    st.live
                        .lock()
                        .expect("live lock")
                        .partials
                        .retain(|x| !(x.channel == channel && x.segment_id == segment_id));
                    UiEvent::Dropped { channel, segment_id }
                }
                MeetingEvent::Level { channel, rms } => UiEvent::Level { channel, rms },
                MeetingEvent::CaptureError { channel, message } => {
                    let label = if channel == Channel::Me { "Microphone" } else { "System audio" };
                    UiEvent::Warning { message: format!("{label}: {message}") }
                }
                MeetingEvent::Error { message, .. } => UiEvent::Warning { message },
            };
            if let UiEvent::Warning { message } = &ui {
                st.live.lock().expect("live lock").warning = Some(message.clone());
            }
            let _ = app.emit(MEETING_EVENT, ui);
        }
    });
}

pub fn pause_meeting(app: &AppHandle) -> Result<(), String> {
    let st = app.state::<AppState>();
    if let Session::Recording(m) = &mut *st.session.lock().expect("session lock") {
        m.pause().map_err(|e| e.to_string())?;
    }
    st.emit_state(app);
    Ok(())
}

pub fn resume_meeting(app: &AppHandle) {
    let st = app.state::<AppState>();
    if let Session::Recording(m) = &mut *st.session.lock().expect("session lock") {
        m.resume();
    }
    st.emit_state(app);
}

pub fn toggle_pause(app: &AppHandle) -> Result<(), String> {
    match app.state::<AppState>().phase() {
        Phase::Recording => pause_meeting(app),
        Phase::Paused => {
            resume_meeting(app);
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn set_language(app: &AppHandle, mode: LanguageMode) {
    let st = app.state::<AppState>();
    if let Session::Recording(m) = &*st.session.lock().expect("session lock") {
        m.set_language(mode);
    }
    {
        let mut cfg = st.config.lock().expect("config lock");
        cfg.language = lang_code(mode).into();
        if let Err(e) = cfg.save() {
            log::error!("saving config: {e}");
        }
    }
    st.emit_state(app);
}

/// Stops capture and asks for a name (SPEC 8.3). Without an answer the meeting is saved under
/// the timestamp name after `name_prompt_seconds`.
pub fn stop_meeting(app: &AppHandle) -> Result<(), String> {
    let st = app.state::<AppState>();
    let meeting = {
        let mut session = st.session.lock().expect("session lock");
        match std::mem::replace(&mut *session, Session::Idle) {
            Session::Recording(m) => m,
            other => {
                *session = other;
                return Ok(());
            }
        }
    };
    let stopped = meeting.stop().map_err(|e| e.to_string())?;
    let token = {
        let mut t = st.naming_token.lock().expect("token lock");
        *t += 1;
        *t
    };
    {
        let mut live = st.live.lock().expect("live lock");
        live.partials.clear();
        live.started = None;
    }
    *st.session.lock().expect("session lock") = Session::Naming { stopped, token };
    st.emit_state(app);
    windows::show_meeting(app);
    windows::focus_for_typing(app, true);
    // The window runs the visible countdown (and extends it while the user types); this is the
    // safety net for a window that never answers.
    let seconds = st.config().meetings.name_prompt_seconds.max(1) as u64 * 2 + 30;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        let st = app.state::<AppState>();
        let still_waiting =
            matches!(&*st.session.lock().expect("session lock"), Session::Naming { token: t, .. } if *t == token);
        if still_waiting {
            log::info!("name prompt timed out, saving with the timestamp name");
            if let Err(e) = save_meeting(&app, None) {
                log::error!("saving meeting: {e}");
            }
        }
    });
    Ok(())
}

/// [`stop_meeting`] off the calling thread (the final decodes take a moment), for the tray and
/// the command line, which run on the event loop.
pub fn stop_meeting_in_background(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = stop_meeting(&app) {
            log::error!("stopping meeting: {e}");
            crate::notify_error(&app, &e);
        }
    });
}

pub fn preview_path(app: &AppHandle, title: Option<&str>) -> Option<String> {
    let st = app.state::<AppState>();
    match &*st.session.lock().expect("session lock") {
        Session::Naming { stopped, .. } => Some(stopped.preview_path(title).display().to_string()),
        _ => None,
    }
}

/// Saves the stopped meeting (an empty title uses the timestamp name).
pub fn save_meeting(app: &AppHandle, title: Option<String>) -> Result<String, String> {
    let st = app.state::<AppState>();
    let stopped = {
        let mut session = st.session.lock().expect("session lock");
        match std::mem::replace(&mut *session, Session::Idle) {
            Session::Naming { stopped, .. } => stopped,
            other => {
                *session = other;
                return Err("no meeting waiting to be saved".into());
            }
        }
    };
    windows::focus_for_typing(app, false);
    let path = stopped.save(title).map_err(|e| e.to_string())?;
    let path = path.display().to_string();
    st.emit_state(app);
    let _ = app.emit(MEETING_EVENT, UiEvent::Saved { path: path.clone() });
    Ok(path)
}

/// On quit: stop and save whatever is running under the timestamp name.
pub fn shutdown(app: &AppHandle) {
    let phase = app.state::<AppState>().phase();
    if matches!(phase, Phase::Recording | Phase::Paused) && stop_meeting(app).is_err() {
        return;
    }
    if app.state::<AppState>().phase() == Phase::Naming {
        let _ = save_meeting(app, None);
    }
}

/// Short microphone (or system audio) test for onboarding: returns the peak level and the
/// transcript of what was heard.
pub fn audio_test(app: &AppHandle, channel: Channel, seconds: f32) -> Result<(f32, String), String> {
    let st = app.state::<AppState>();
    st.ensure_runtime()?;
    let factory = sources::source(channel).ok_or("not available on this platform")?;
    let mut source = factory().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    source.start(tx).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs_f32(seconds);
    let mut chunks = Vec::new();
    while Instant::now() < deadline {
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(100)) {
            chunks.push(c);
        }
    }
    let _ = source.stop();
    let mut pipeline = {
        let rt = st.runtime.lock().expect("runtime lock");
        rt.as_ref().ok_or("models not loaded")?.pipeline(channel, false).map_err(|e| e.to_string())?
    };
    let mut peak = 0.0f32;
    let mut text = Vec::new();
    let mut resampler: Option<(u32, tyst_core::resample::Resampler)> = None;
    let mut collect = |events: Vec<tyst_core::pipeline::PipelineEvent>| {
        for e in events {
            if let tyst_core::pipeline::PipelineEvent::Final { segment, .. } = e {
                text.push(segment.text);
            }
        }
    };
    for c in chunks {
        let r = match &mut resampler {
            Some((rate, r)) if *rate == c.sample_rate => r,
            _ => &mut resampler.insert((c.sample_rate, tyst_core::resample::Resampler::new(c.sample_rate, 16_000))).1,
        };
        let pcm = r.push(&c.samples);
        peak = pcm.iter().fold(peak, |m, x| m.max(x.abs()));
        collect(pipeline.push(&pcm).map_err(|e| e.to_string())?);
    }
    collect(pipeline.flush().map_err(|e| e.to_string())?);
    Ok((peak, text.join(" ")))
}
