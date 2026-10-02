//! End-to-end checks with the real models. Skipped unless the models are installed in
//! `$TYST_MODELS` (see `tyst-cli models fetch`). With `TYST_TEST_CLIP=<audio>` (and a reference
//! transcript next to it as `<audio stem>.txt`), the clip's WER is checked too.

use std::path::{Path, PathBuf};

use tyst_core::asr::AsrEngine;
use tyst_core::asr::onnx::{OnnxModelFiles, OnnxTdtEngine, SessionOptions};
use tyst_core::models::{self, Manifest, PIANISSIMO, SILERO_VAD};
use tyst_core::segmenter::{Segmenter, SegmenterConfig, SegmenterEvent};
use tyst_core::text::word_errors;
use tyst_core::transcript::Lang;
use tyst_core::vad::SileroVad;

fn installed(id: &str) -> Option<PathBuf> {
    std::env::var_os("TYST_MODELS")?;
    let manifest = Manifest::builtin();
    let spec = manifest.get(id).ok()?;
    models::installed_dir(spec, &models::default_models_dir()).ok()
}

fn engine() -> Option<OnnxTdtEngine> {
    let dir = installed(PIANISSIMO)?;
    let files = OnnxModelFiles::discover(&dir).unwrap();
    Some(OnnxTdtEngine::load("pianissimo", &files, SessionOptions { threads: 2 }).unwrap())
}

#[test]
fn silence_decodes_to_nothing() {
    let Some(mut e) = engine() else {
        eprintln!("skipped: models not installed");
        return;
    };
    let r = e.transcribe(&vec![0.0; 16_000]).unwrap();
    assert!(r.text.is_empty(), "{:?}", r.text);
}

#[test]
fn vad_finds_no_speech_in_silence() {
    let Some(dir) = installed(SILERO_VAD) else {
        eprintln!("skipped: models not installed");
        return;
    };
    let manifest = Manifest::builtin();
    let vad = SileroVad::load(&dir.join(&manifest.get(SILERO_VAD).unwrap().files[0].name)).unwrap();
    let mut seg = Segmenter::new(SegmenterConfig::default(), vad);
    let mut events = seg.push(&vec![0.0; 48_000]).unwrap();
    events.extend(seg.flush());
    assert!(!events.iter().any(|e| matches!(e, SegmenterEvent::Ended(_))));
}

#[test]
fn clip_from_env_transcribes() {
    let Some(clip) = std::env::var_os("TYST_TEST_CLIP").map(PathBuf::from) else {
        eprintln!("skipped: TYST_TEST_CLIP not set");
        return;
    };
    let mut e = engine().expect("TYST_TEST_CLIP needs the models installed");
    let pcm = tyst_core::audio_file::load_16k_mono(&clip).unwrap();
    let text = e.transcribe(&pcm).unwrap().text;
    let reference = std::fs::read_to_string(Path::new(&clip).with_extension("txt")).unwrap();
    let (errors, words) = word_errors(&reference, &text, Some(Lang::Sv));
    let wer = errors as f64 / words.max(1) as f64;
    assert!(wer < 0.3, "WER {wer:.2}");
}
