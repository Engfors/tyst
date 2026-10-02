//! Audio files to 16 kHz mono f32 (WAV, FLAC, MP3, Ogg Vorbis, M4A/AAC).

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::resample::{Resampler, SAMPLE_RATE, downmix};
use crate::{Error, Result};

/// Decoded audio at its original rate, downmixed to mono.
pub struct MonoAudio {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

/// Decodes a file and converts it to 16 kHz mono, like the Phase 0 harness's `load_audio`.
pub fn load_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let audio = decode_mono(path)?;
    Ok(Resampler::resample(&audio.samples, audio.sample_rate, SAMPLE_RATE))
}

pub fn decode_mono(path: &Path) -> Result<MonoAudio> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let bad = |e: SymError| Error::Audio(format!("{}: {e}", path.display()));
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(bad)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Audio(format!("{}: no audio track", path.display())))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| Error::Audio(format!("{}: not an audio track", path.display())))?;
    let mut decoder =
        symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).map_err(bad)?;

    let mut mono = Vec::new();
    let mut rate = 0;
    let mut interleaved: Vec<f32> = Vec::new();
    while let Some(packet) = format.next_packet().map_err(bad)? {
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                rate = buf.spec().rate();
                let channels = buf.spec().channels().count().max(1);
                interleaved.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut interleaved);
                mono.extend(downmix(&interleaved, channels));
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(bad(e)),
        }
    }
    if rate == 0 {
        return Err(Error::Audio(format!("{}: no audio decoded", path.display())));
    }
    Ok(MonoAudio { sample_rate: rate, samples: mono })
}

/// Writes 16-bit PCM WAV (test fixtures and debugging; never used for user audio).
pub fn write_wav_16bit(path: &Path, samples: &[f32], sample_rate: u32) -> Result<()> {
    let mut data = Vec::with_capacity(44 + samples.len() * 2);
    let bytes = (samples.len() * 2) as u32;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + bytes).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&sample_rate.to_le_bytes());
    data.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        data.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, data).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip_and_resample() {
        let dir = std::env::temp_dir().join(format!("tyst-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let tone: Vec<f32> = (0..48_000).map(|i| 0.5 * (i as f32 * 0.01).sin()).collect();
        write_wav_16bit(&path, &tone, 48_000).unwrap();
        let decoded = decode_mono(&path).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.samples.len(), 48_000);
        assert!((decoded.samples[1000] - tone[1000]).abs() < 1e-3);
        assert_eq!(load_16k_mono(&path).unwrap().len(), 16_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
