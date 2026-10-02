//! Microphone capture through cpal: the default input device, any native rate and format,
//! downmixed to mono. Resampling to 16 kHz happens in the core.

use std::sync::mpsc::Sender;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};

use crate::{AudioChunk, AudioSource, CaptureError, Channel};

pub struct MicSource {
    stream: Option<Stream>,
    sample_rate: u32,
    device_name: String,
}

impl MicSource {
    pub fn new() -> Self {
        Self { stream: None, sample_rate: 0, device_name: String::new() }
    }
}

impl Default for MicSource {
    fn default() -> Self {
        Self::new()
    }
}

fn to_mono<T: Copy>(data: &[T], channels: usize, conv: impl Fn(T) -> f32) -> Vec<f32> {
    let channels = channels.max(1);
    data.chunks_exact(channels).map(|f| f.iter().map(|&s| conv(s)).sum::<f32>() / channels as f32).collect()
}

impl AudioSource for MicSource {
    fn start(&mut self, sink: Sender<AudioChunk>) -> Result<(), CaptureError> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or(CaptureError::NoDevice)?;
        self.device_name = device.id().map(|id| id.to_string()).unwrap_or_else(|_| "default input".into());
        let config = device.default_input_config().map_err(|e| CaptureError::Device(e.to_string()))?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        self.sample_rate = rate;
        let format = config.sample_format();
        let err_fn = |e: cpal::Error| log::warn!("microphone stream error: {e}");
        let send = move |samples: Vec<f32>| {
            let _ =
                sink.send(AudioChunk { channel: Channel::Me, sample_rate: rate, samples, captured_at: Instant::now() });
        };
        let stream = match format {
            SampleFormat::F32 => device.build_input_stream(
                config.into(),
                move |data: &[f32], _: &_| send(to_mono(data, channels, |s| s)),
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config.into(),
                move |data: &[i16], _: &_| send(to_mono(data, channels, |s| s as f32 / 32768.0)),
                err_fn,
                None,
            ),
            SampleFormat::I32 => device.build_input_stream(
                config.into(),
                move |data: &[i32], _: &_| send(to_mono(data, channels, |s| s as f32 / 2_147_483_648.0)),
                err_fn,
                None,
            ),
            other => return Err(CaptureError::Device(format!("unsupported sample format {other}"))),
        }
        .map_err(|e| CaptureError::Device(e.to_string()))?;
        stream.play().map_err(|e| CaptureError::Device(e.to_string()))?;
        log::info!("microphone started: {rate} Hz, {channels} channel(s), {format}");
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        self.stream = None;
        Ok(())
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }
}
