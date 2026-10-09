//! Parity with the Python reference code, from fixtures written by `fixtures/make_golden.py`.
//!
//! The boosting tree is checked on every run. The tokenizer check needs Pianissimo's
//! `tokenizer.model`: `TYST_TOKENIZER=<path> cargo test -p tyst-core --test boost -- --ignored`
//! (or the file in `$TYST_MODELS/pianissimo-sv-onnx/`). `TYST_SPM_GOLDEN` points it at a larger
//! fixture generated with the same script.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tyst_core::asr::boost::{BoostTree, CONTEXT_SCORE, DEPTH_SCALING, PhraseBoost};
use tyst_core::asr::spm::SpmEncoder;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[derive(Deserialize)]
struct BoostCase {
    vocab: u32,
    phrases: Vec<Vec<u32>>,
    states: Vec<StateScores>,
}

#[derive(Deserialize)]
struct StateScores {
    bonus: Vec<f32>,
    next: Vec<u32>,
}

#[test]
fn boost_tree_matches_klang() {
    let cases: Vec<BoostCase> =
        serde_json::from_str(&std::fs::read_to_string(fixture("boost_golden.json")).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for (n, case) in cases.iter().enumerate() {
        let tree = BoostTree::new(&case.phrases, CONTEXT_SCORE, DEPTH_SCALING);
        assert_eq!(tree.len(), case.states.len(), "case {n}: state count");
        for (state, want) in case.states.iter().enumerate() {
            let scores = tree.scores(state as u32);
            for token in 0..case.vocab {
                let (bonus, next) = scores.get(token);
                let i = token as usize;
                assert_eq!(next, want.next[i], "case {n} state {state} token {token}: next");
                assert!((bonus - want.bonus[i]).abs() < 1e-5, "case {n} state {state} token {token}: {bonus}");
            }
        }
    }
}

/// The sparse argmax picks what Klang's dense `argmax(logits[:blank] + alpha * bonus)` picks.
#[test]
fn boosted_choice_matches_dense_argmax() {
    let cases: Vec<BoostCase> =
        serde_json::from_str(&std::fs::read_to_string(fixture("boost_golden.json")).unwrap()).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    for case in &cases {
        for alpha in [0.25f32, 0.5, 1.0, 2.0] {
            let boost = PhraseBoost { tree: BoostTree::new(&case.phrases, CONTEXT_SCORE, DEPTH_SCALING), alpha };
            let blank = case.vocab;
            let mut state = 0u32;
            for _ in 0..50 {
                let logits: Vec<f32> = (0..=blank).map(|_| rand() * 6.0).collect();
                let want = &case.states[state as usize];
                let mut best = 0usize;
                for i in 0..blank as usize {
                    if logits[i] + alpha * want.bonus[i] > logits[best] + alpha * want.bonus[best] {
                        best = i;
                    }
                }
                let got = boost.choose(&logits, blank, &mut state);
                assert_eq!(got as usize, best);
                assert_eq!(state, want.next[best]);
            }
        }
    }
}

fn tokenizer() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TYST_TOKENIZER") {
        return Some(p.into());
    }
    let p = PathBuf::from(std::env::var_os("TYST_MODELS")?).join("pianissimo-sv-onnx/tokenizer.model");
    p.is_file().then_some(p)
}

#[derive(Deserialize)]
struct SpmCase {
    text: String,
    ids: Vec<u32>,
}

#[test]
#[ignore = "needs Pianissimo's tokenizer.model in $TYST_TOKENIZER or $TYST_MODELS"]
fn spm_encoder_matches_sentencepiece() {
    let Some(path) = tokenizer() else {
        eprintln!("skipped: no tokenizer.model");
        return;
    };
    let encoder = SpmEncoder::load(&path).unwrap();
    let golden = std::env::var_os("TYST_SPM_GOLDEN").map_or_else(|| fixture("spm_golden.jsonl"), PathBuf::from);
    let text = std::fs::read_to_string(golden).unwrap();
    let mut failures = Vec::new();
    let mut total = 0;
    for line in text.lines() {
        let case: SpmCase = serde_json::from_str(line).unwrap();
        total += 1;
        let got = encoder.encode(&case.text);
        if got != case.ids {
            failures.push(format!("{:?}: got {got:?}, want {:?}", case.text, case.ids));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {total} differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
    eprintln!("{total} strings match");
}
