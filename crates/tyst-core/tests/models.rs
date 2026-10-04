//! End-to-end checks with the real models, ignored by default so CI reports them as ignored rather
//! than passed. Run them with the models installed in `$TYST_MODELS` (see `tyst-cli models fetch`):
//! `TYST_MODELS=... cargo test -p tyst-core --test models -- --ignored`. With `TYST_TEST_CLIP=<audio>` (and a reference
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
#[ignore = "needs the real models in $TYST_MODELS"]
fn silence_decodes_to_nothing() {
    let Some(mut e) = engine() else {
        eprintln!("skipped: models not installed");
        return;
    };
    let r = e.transcribe(&vec![0.0; 16_000]).unwrap();
    assert!(r.text.is_empty(), "{:?}", r.text);
}

#[test]
#[ignore = "needs the real models in $TYST_MODELS"]
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
#[ignore = "needs the real models in $TYST_MODELS"]
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

/// The banded encoder that `models fetch` derives must transcribe exactly like the downloaded one.
#[test]
#[ignore = "needs the real models in $TYST_MODELS"]
fn banded_encoder_matches_the_original() {
    let Some(dir) = installed(PIANISSIMO) else {
        eprintln!("skipped: models not installed");
        return;
    };
    let banded = OnnxModelFiles::discover(&dir).unwrap();
    assert!(banded.encoder.ends_with("encoder-model.banded.int8.onnx"), "{}", banded.encoder.display());
    let original = OnnxModelFiles { encoder: dir.join("encoder-model.int8.onnx"), ..banded.clone() };
    let opts = SessionOptions { threads: 2 };
    let mut a = OnnxTdtEngine::load("original", &original, opts).unwrap();
    let mut b = OnnxTdtEngine::load("banded", &banded, opts).unwrap();
    let pcm = match std::env::var_os("TYST_TEST_CLIP") {
        Some(clip) => tyst_core::audio_file::load_16k_mono(Path::new(&clip)).unwrap(),
        None => (0..16_000 * 30).map(|i| (i as f32 * 0.07).sin() * 0.1).collect(),
    };
    // Shorter and longer than the 256-frame (20.5 s) attention window.
    for secs in [1usize, 4, 15, 25] {
        let clip = &pcm[..(secs * 16_000).min(pcm.len())];
        let (ra, rb) = (a.transcribe(clip).unwrap(), b.transcribe(clip).unwrap());
        assert_eq!(ra.text, rb.text, "{secs} s");
        assert_eq!(ra.tokens, rb.tokens, "{secs} s");
    }
}
