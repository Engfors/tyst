//! `tyst-cli echo-cancel`: runs the meeting's echo cancellation (SPEC 6.4) offline on a
//! microphone recording with the system audio that played at the same time, and writes the
//! cleaned microphone track. For checking the canceller on real recordings.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
use tyst_core::audio_file;
use tyst_core::resample::SAMPLE_RATE;
use tyst_runtime::echo::{EchoCanceller, EchoReference};

#[derive(Args)]
pub struct EchoArgs {
    /// Microphone recording (Me), with the speakers' echo in it.
    #[arg(long)]
    pub mic: PathBuf,
    /// System audio recorded at the same time (Others), starting at the same moment.
    #[arg(long)]
    pub system: PathBuf,
    /// Where to write the cleaned microphone track (16 kHz WAV).
    #[arg(long)]
    pub out: PathBuf,
}

pub fn run(args: EchoArgs) -> Result<()> {
    let mic = audio_file::load_16k_mono(&args.mic)?;
    let system = audio_file::load_16k_mono(&args.system)?;
    let reference = Arc::new(EchoReference::default());
    let mut aec = EchoCanceller::new(reference.clone()).map_err(anyhow::Error::msg)?;
    // 20 ms chunks, the reference always just ahead, as in a meeting.
    let chunk = SAMPLE_RATE as usize / 50;
    let mut out = Vec::with_capacity(mic.len());
    for (i, m) in mic.chunks(chunk).enumerate() {
        let at = (i * chunk) as u64;
        let start = (i * chunk).min(system.len());
        let end = (start + chunk).min(system.len());
        reference.push(at, &system[start..end]);
        for (_, block) in aec.push(at, m, at + chunk as u64) {
            out.extend(block);
        }
    }
    for (_, block) in aec.flush() {
        out.extend(block);
    }
    audio_file::write_wav_16bit(&args.out, &out, SAMPLE_RATE)?;
    let power = |x: &[f32]| x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    eprintln!(
        "wrote {} ({:.1} s); level change {:.1} dB; echo delay {}",
        args.out.display(),
        out.len() as f64 / SAMPLE_RATE as f64,
        10.0 * (power(&out) / power(&mic).max(1e-12)).log10(),
        aec.delay_ms().map(|d| format!("{d} ms")).unwrap_or_else(|| "not found".into())
    );
    Ok(())
}
