//! Linux capture through PipeWire (SPEC 10.2): the default source for Me, and the monitor of the
//! default sink for Others (everything the computer plays).
//!
//! Streams are auto-connected without a target, so the session manager (WirePlumber) links them
//! to the current default device and moves them when the default changes or a headset is
//! unplugged; the session keeps running across device switches. PipeWire converts to mono f32 at
//! the graph rate; resampling to 16 kHz happens in the core.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::param::format::{MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::pod::Pod;

use crate::{AudioChunk, AudioSource, CaptureError, Channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The default input (microphone).
    DefaultSource,
    /// The monitor of the default output: system audio.
    DefaultSinkMonitor,
}

pub struct PipeWireSource {
    channel: Channel,
    target: Target,
    rate: Arc<AtomicU32>,
    /// Set by the stream when it fails; `poll` restarts it.
    failed: Arc<Mutex<Option<String>>>,
    running: Option<Running>,
    sink: Option<Sender<AudioChunk>>,
}

struct Running {
    quit: pw::channel::Sender<()>,
    thread: JoinHandle<()>,
}

impl PipeWireSource {
    pub fn new(channel: Channel, target: Target) -> Self {
        Self { channel, target, rate: Arc::new(AtomicU32::new(0)), failed: Arc::default(), running: None, sink: None }
    }

    /// Microphone (Me).
    pub fn microphone() -> Self {
        Self::new(Channel::Me, Target::DefaultSource)
    }

    /// System audio (Others).
    pub fn system_audio() -> Self {
        Self::new(Channel::Others, Target::DefaultSinkMonitor)
    }

    fn spawn(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let (channel, target, rate, failed) = (self.channel, self.target, self.rate.clone(), self.failed.clone());
        *failed.lock().expect("lock") = None;
        let thread = std::thread::Builder::new()
            .name(format!("tyst-pipewire-{channel:?}"))
            .spawn(move || {
                let result = run_stream(channel, target, sink, rate, failed, quit_rx, &ready_tx);
                if let Err(e) = result {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| CaptureError::Device(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {
                self.running = Some(Running { quit: quit_tx, thread });
                Ok(())
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(CaptureError::Device(format!("pipewire: {e}")))
            }
            Err(_) => {
                let _ = quit_tx.send(());
                Err(CaptureError::Device("pipewire: no answer from the PipeWire daemon".into()))
            }
        }
    }
}

impl AudioSource for PipeWireSource {
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        self.stop()?;
        self.sink = Some(sink.clone());
        self.spawn(sink)
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        if let Some(r) = self.running.take() {
            let _ = r.quit.send(());
            let _ = r.thread.join();
        }
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.rate.load(Ordering::SeqCst)
    }

    fn device_name(&self) -> String {
        match self.target {
            Target::DefaultSource => "PipeWire default source".into(),
            Target::DefaultSinkMonitor => "PipeWire default sink monitor".into(),
        }
    }

    /// Restarts the stream after an error (e.g. the daemon restarted).
    fn poll(&mut self) -> Result<(), CaptureError> {
        let failure = self.failed.lock().expect("lock").take();
        let Some(reason) = failure else { return Ok(()) };
        log::warn!("{:?} PipeWire stream failed ({reason}), restarting", self.channel);
        self.stop()?;
        match self.sink.clone() {
            Some(sink) => self.spawn(sink),
            None => Ok(()),
        }
    }
}

impl Drop for PipeWireSource {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct StreamData {
    format: spa::param::audio::AudioInfoRaw,
    channel: Channel,
    sink: Sender<AudioChunk>,
    rate: Arc<AtomicU32>,
}

fn run_stream(
    channel: Channel,
    target: Target,
    sink: Sender<AudioChunk>,
    rate: Arc<AtomicU32>,
    failed: Arc<Mutex<Option<String>>>,
    quit: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;

    let (node, role) = match target {
        Target::DefaultSource => ("tyst-microphone", "Communication"),
        Target::DefaultSinkMonitor => ("tyst-system-audio", "Communication"),
    };
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => role,
        *pw::keys::APP_NAME => "Tyst",
        *pw::keys::NODE_NAME => node,
        *pw::keys::NODE_LATENCY => "1024/48000",
    };
    if target == Target::DefaultSinkMonitor {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }
    let stream = pw::stream::StreamBox::new(&core, node, props).map_err(|e| e.to_string())?;

    let data = StreamData { format: Default::default(), channel, sink, rate };
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(move |_, _, _, new| {
            if let pw::stream::StreamState::Error(e) = new {
                log::warn!("{channel:?} PipeWire stream error: {e}");
                *failed.lock().expect("lock") = Some(e);
            }
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((MediaType::Audio, MediaSubtype::Raw)) = format_utils::parse_format(param) else { return };
            if data.format.parse(param).is_ok() {
                data.rate.store(data.format.rate(), Ordering::SeqCst);
                log::info!(
                    "{:?} PipeWire format: {} Hz, {} channel(s)",
                    data.channel,
                    data.format.rate(),
                    data.format.channels()
                );
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let channels = data.format.channels().max(1) as usize;
            let rate = data.format.rate();
            let size = d.chunk().size() as usize;
            let offset = d.chunk().offset() as usize;
            let Some(bytes) = d.data() else { return };
            let end = (offset + size).min(bytes.len());
            if rate == 0 || end <= offset {
                return;
            }
            let samples: Vec<f32> =
                bytes[offset..end].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
            let frames: Vec<f32> =
                samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
            let _ = data.sink.send(AudioChunk {
                channel: data.channel,
                sample_rate: rate,
                samples: frames,
                captured_at: Instant::now(),
            });
        })
        .register()
        .map_err(|e| e.to_string())?;

    // Mono f32; rate left open, so the graph rate is used.
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_channels(1);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let values: Vec<u8> =
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
            .map_err(|e| format!("{e:?}"))?
            .0
            .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or("format pod")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}
