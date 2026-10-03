//! An [`AudioSource`] that plays an audio file in real time, as if it were being captured. For
//! headless tests of live sessions (`tyst-cli live --simulate`, `tyst-cli meeting --me/--others`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tyst_core::audio_file::MonoAudio;
use tyst_platform::{AudioChunk, AudioSource, CaptureError, Channel};

pub struct FilePlayback {
    channel: Channel,
    audio: Arc<MonoAudio>,
    speed: f64,
    name: String,
    /// Samples already played, so a pause resumes where it stopped.
    position: Arc<std::sync::atomic::AtomicUsize>,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FilePlayback {
    pub fn open(path: &Path, channel: Channel, speed: f64) -> Result<Self, CaptureError> {
        let audio = tyst_core::audio_file::decode_mono(path).map_err(|e| CaptureError::Device(e.to_string()))?;
        let name = format!("file:{}", path.file_name().unwrap_or_default().to_string_lossy());
        Ok(Self::from_audio(audio, channel, speed, name))
    }

    /// Plays audio already in memory.
    pub fn from_audio(audio: MonoAudio, channel: Channel, speed: f64, name: String) -> Self {
        Self {
            channel,
            audio: Arc::new(audio),
            speed,
            name,
            position: Arc::default(),
            stop: Arc::default(),
            finished: Arc::default(),
            thread: None,
        }
    }

    /// True once the whole file has been played.
    pub fn finished_flag(&self) -> Arc<AtomicBool> {
        self.finished.clone()
    }
}

impl AudioSource for FilePlayback {
    /// Sends the file in 20 ms chunks paced by the wall clock, like a capture callback would.
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        self.stop.store(false, Ordering::SeqCst);
        let (audio, speed, channel) = (self.audio.clone(), self.speed, self.channel);
        let (position, stop, finished) = (self.position.clone(), self.stop.clone(), self.finished.clone());
        self.thread = Some(std::thread::spawn(move || {
            let chunk = (audio.sample_rate / 50) as usize;
            let t0 = Instant::now();
            let first = position.load(Ordering::SeqCst);
            let mut i = 0usize;
            while !stop.load(Ordering::SeqCst) {
                let from = first + i * chunk;
                if from >= audio.samples.len() {
                    finished.store(true, Ordering::SeqCst);
                    return;
                }
                let piece = &audio.samples[from..(from + chunk).min(audio.samples.len())];
                i += 1;
                let due = t0 + Duration::from_secs_f64((i * chunk) as f64 / audio.sample_rate as f64 / speed);
                let now = Instant::now();
                if due > now {
                    std::thread::sleep(due - now);
                }
                position.store(from + piece.len(), Ordering::SeqCst);
                let c = AudioChunk {
                    channel,
                    sample_rate: audio.sample_rate,
                    samples: piece.to_vec(),
                    captured_at: Instant::now(),
                };
                if sink.send(c).is_err() {
                    return;
                }
            }
        }));
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.audio.sample_rate
    }

    fn device_name(&self) -> String {
        self.name.clone()
    }
}
