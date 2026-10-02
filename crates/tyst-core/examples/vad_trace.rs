//! Prints Silero speech probabilities and segment boundaries for a file (timings only).
//!
//! cargo run --release -p tyst-core --example vad_trace -- <silero.onnx> <audio>

use tyst_core::segmenter::{Segmenter, SegmenterConfig, SegmenterEvent, SpeechDetector};
use tyst_core::vad::SileroVad;

fn main() -> tyst_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let pcm = tyst_core::audio_file::load_16k_mono(args[2].as_ref())?;
    let mut probe = SileroVad::load(args[1].as_ref())?;
    let mut line = String::new();
    for (i, w) in pcm.as_chunks::<512>().0.iter().enumerate() {
        let p = probe.prob(w)?;
        line.push(if p > 0.5 {
            '#'
        } else if p > 0.35 {
            '+'
        } else {
            '.'
        });
        if (i + 1) % 125 == 0 {
            println!("{:>6.1}s {line}", (i + 1) as f32 * 0.032 - 4.0);
            line.clear();
        }
    }
    let mut seg = Segmenter::new(SegmenterConfig::default(), SileroVad::load(args[1].as_ref())?);
    let mut events = seg.push(&pcm)?;
    events.extend(seg.flush());
    for e in events {
        match e {
            SegmenterEvent::Started { start } => println!("start {:.2}", start as f32 / 16000.0),
            SegmenterEvent::Ended(s) => {
                println!("end   {:.2} -> {:.2} forced={}", s.start as f32 / 16000.0, s.end() as f32 / 16000.0, s.forced)
            }
            SegmenterEvent::Dropped { start, end } => {
                println!("drop  {:.2} -> {:.2}", start as f32 / 16000.0, end as f32 / 16000.0)
            }
        }
    }
    Ok(())
}
