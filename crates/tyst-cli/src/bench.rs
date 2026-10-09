//! `tyst-cli bench`: scores a clip manifest the way the Phase 0 harness does (`only-sv` strategy,
//! greedy decoding), so Phase 1 can check it lands within 1 pp of the harness WER (SPEC 12).
//!
//! Writes `summary.md` / `summary.json` (metrics only, shareable) and `hyp/` (transcripts, not
//! shareable) under `eval/results/rust-<timestamp>/` unless `--out` says otherwise.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};
use tyst_core::pipeline::PipelineEvent;
use tyst_core::router::LanguageMode;
use tyst_core::text::{term_hits, word_errors};
use tyst_core::transcript::{Channel, Lang};
use tyst_core::vocabulary::{self, BOOST_STRENGTH, MAX_BOOST_STRENGTH, VocabularyFile, VocabularyRules};
use tyst_runtime::Runtime;

use crate::stats;
use crate::{EngineArgs, setup};

#[derive(Args)]
pub struct BenchArgs {
    /// Clip manifest (e.g. eval/manifest.toml); clips missing on disk are skipped.
    pub manifest: PathBuf,
    /// Only these categories (comma-separated), e.g. sv,en,mixed,sv-terms.
    #[arg(long)]
    pub categories: Option<String>,
    /// Only the first N clips.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Terms for term accuracy and the `vocab` rows (default: terms.toml next to the manifest).
    #[arg(long)]
    pub terms: Option<PathBuf>,
    /// Harness `summary.json` to compare against (its greedy / only-sv rows).
    #[arg(long)]
    pub baseline: Option<PathBuf>,
    /// Also decode with phrase boosting at these strengths (comma-separated, above 0 and at most 1,
    /// e.g. 0.25,0.5,1), boosting the terms and joining replacements of the terms file. Adds
    /// `boost <s>` and `boost <s>+vocab` rows. Needs `tokenizer.model` (`tyst-cli models fetch`).
    /// With `--lang en` this boosts Parakeet, which the app does not do yet (English eval).
    #[arg(long, value_delimiter = ',')]
    pub boost: Vec<f32>,
    /// Do not spell out digits before scoring (harness `--no-number-norm`).
    #[arg(long)]
    pub no_number_norm: bool,
    /// Output directory.
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[command(flatten)]
    pub engine: EngineArgs,
}

// --------------------------------------------------------------------------- manifest

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    settings: Settings,
    #[serde(default)]
    include: Vec<Include>,
    #[serde(default)]
    clip: Vec<ClipEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct Settings {
    clips_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Include {
    path: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ClipEntry {
    id: String,
    category: String,
    audio: String,
    reference: String,
    #[serde(default = "default_lang")]
    lang: String,
}

fn default_lang() -> String {
    "sv".into()
}

#[derive(Debug, Clone)]
struct Clip {
    id: String,
    category: String,
    audio: PathBuf,
    reference: PathBuf,
    lang: Lang,
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// Same resolution as the harness: `TYST_EVAL_CLIPS`, else `settings.clips_root`, paths relative to it.
fn load_manifest(path: &Path) -> Result<(Vec<Clip>, Vec<String>, PathBuf)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let m: ManifestFile = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let root = std::env::var("TYST_EVAL_CLIPS")
        .ok()
        .or(m.settings.clips_root.clone())
        .map(|r| expand(&r))
        .unwrap_or_else(|| expand("~/tyst-eval/clips"));
    let mut entries: Vec<(ClipEntry, PathBuf)> = m.clip.into_iter().map(|c| (c, root.clone())).collect();
    let mut warnings = Vec::new();
    for inc in m.include {
        let inc_path = root.join(&inc.path);
        let Ok(t) = std::fs::read_to_string(&inc_path) else {
            warnings.push(format!("include not found (skipped): {}", inc_path.display()));
            continue;
        };
        let im: ManifestFile = toml::from_str(&t).with_context(|| format!("parsing {}", inc_path.display()))?;
        let base = inc_path.parent().unwrap_or(&root).to_path_buf();
        entries.extend(im.clip.into_iter().map(|c| (c, base.clone())));
    }
    let mut clips = Vec::new();
    for (c, base) in entries {
        let clip = Clip {
            audio: base.join(expand(&c.audio)),
            reference: base.join(expand(&c.reference)),
            lang: Lang::parse(&c.lang).unwrap_or(Lang::Sv),
            id: c.id,
            category: c.category,
        };
        if clip.audio.is_file() && clip.reference.is_file() {
            clips.push(clip);
        } else {
            warnings.push(format!("clip {}: audio or reference missing (skipped)", clip.id));
        }
    }
    Ok((clips, warnings, root))
}

// --------------------------------------------------------------------------- scoring

#[derive(Debug, Default, Clone, Serialize)]
struct Acc {
    clips: usize,
    errors: usize,
    words: usize,
    term_found: usize,
    term_exact: usize,
    term_total: usize,
    decode_s: f64,
    speech_s: f64,
}

#[derive(Debug, Serialize)]
struct Row {
    post: String,
    category: String,
    clips: usize,
    wer: Option<f64>,
    term_recall: Option<f64>,
    term_exact: Option<f64>,
    rtf: Option<f64>,
    baseline_wer: Option<f64>,
}

#[derive(Debug, Serialize)]
struct Summary {
    time: String,
    platform: String,
    threads: usize,
    engine: Vec<String>,
    clips: BTreeMap<String, ClipInfo>,
    model_load_s: f64,
    rss_mb_after_load: f64,
    rss_mb_peak: f64,
    engine_rtf: f64,
    rows: Vec<Row>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ClipInfo {
    category: String,
    audio_s: f64,
    segments: usize,
}

#[derive(Deserialize)]
struct BaselineFile {
    rows: Vec<BaselineRow>,
}

#[derive(Deserialize)]
struct BaselineRow {
    decode: String,
    strategy: String,
    post: String,
    category: String,
    wer: Option<f64>,
}

pub fn run(args: BenchArgs) -> Result<()> {
    if let Some(bad) = args.boost.iter().find(|a| !(**a > 0.0 && **a <= MAX_BOOST_STRENGTH)) {
        bail!("--boost {bad}: strengths must be above 0 and at most {MAX_BOOST_STRENGTH}");
    }
    let english = LanguageMode::parse(&args.engine.lang) == Some(LanguageMode::English);
    let (mut clips, warnings, root) = load_manifest(&args.manifest)?;
    if let Some(cats) = &args.categories {
        let wanted: Vec<&str> = cats.split(',').collect();
        clips.retain(|c| wanted.contains(&c.category.as_str()));
    }
    if let Some(n) = args.limit {
        clips.truncate(n);
    }
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if clips.is_empty() {
        bail!("no clips found (clips root: {})", root.display());
    }
    let terms_path = args.terms.clone().unwrap_or_else(|| args.manifest.with_file_name("terms.toml"));
    let terms_file = vocabulary::load_file(&terms_path).with_context(|| format!("loading {}", terms_path.display()))?;
    let vocab = VocabularyRules::new(&terms_file);

    // Raw model output is scored; the vocabulary is applied afterwards for the `vocab` rows,
    // like the harness does.
    let mut engine_args = args.engine.clone();
    engine_args.vocab = None;
    let mut rt = setup::load(&engine_args)?;
    let rss_after_load = stats::current_rss_mb();

    let out_dir = args.out.clone().unwrap_or_else(|| {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        args.manifest.parent().unwrap_or(Path::new(".")).join("results").join(format!("rust-{ts}"))
    });
    let hyp_dir = out_dir.join("hyp");
    std::fs::create_dir_all(&hyp_dir)?;

    let mut acc: BTreeMap<(String, String), Acc> = BTreeMap::new();
    let mut clip_info = BTreeMap::new();
    let (mut decode_total, mut speech_total) = (0.0, 0.0);
    for clip in &clips {
        let t0 = Instant::now();
        let pcm = tyst_core::audio_file::load_16k_mono(&clip.audio)?;
        let reference = std::fs::read_to_string(&clip.reference)?;
        let lang = (!args.no_number_norm).then_some(clip.lang);
        let mut runs = vec![(None, "raw".to_string(), "vocab".to_string())];
        runs.extend(args.boost.iter().map(|a| (Some(*a), format!("boost {a}"), format!("boost {a}+vocab"))));
        let (mut raw, mut decode_s, mut speech_s, mut segments) = (String::new(), 0.0, 0.0, 0);
        for (strength, raw_post, vocab_post) in &runs {
            // Boosted runs decode with the boost only; the text rules are scored separately below.
            let rules = match strength {
                None => VocabularyRules::default(),
                Some(_) => {
                    let rules =
                        VocabularyRules::new(&VocabularyFile { boost: true, ..terms_file.clone() }).boost_only();
                    if english { rules.with_english_boost() } else { rules }
                }
            };
            rt.boost_strength = strength.unwrap_or(BOOST_STRENGTH);
            rt.set_vocabulary(rules);
            if strength.is_some() && !rt.boosting() {
                bail!("--boost needs tokenizer.model and at least one term: run `tyst-cli models fetch`");
            }
            let run = decode(&rt, &pcm)?;
            if strength.is_none() {
                (raw, decode_s, speech_s, segments) = run.clone();
                std::fs::write(hyp_dir.join(format!("{}.txt", clip.id)), format!("{raw}\n"))?;
            } else {
                std::fs::write(hyp_dir.join(format!("{}.{raw_post}.txt", clip.id)), format!("{}\n", run.0))?;
            }
            let (hyp, run_decode_s, run_speech_s, _) = run;
            for (post, hyp) in [(raw_post, hyp.clone()), (vocab_post, vocab.apply(&hyp))] {
                let (errors, words) = word_errors(&reference, &hyp, lang);
                let (found, exact, total) = term_hits(&reference, &hyp, &terms_file.terms);
                for cat in [clip.category.clone(), "ALL".to_string()] {
                    let a = acc.entry((post.to_string(), cat)).or_default();
                    a.clips += 1;
                    a.errors += errors;
                    a.words += words;
                    a.term_found += found;
                    a.term_exact += exact;
                    a.term_total += total;
                    a.decode_s += run_decode_s;
                    a.speech_s += run_speech_s;
                }
            }
        }
        decode_total += decode_s;
        speech_total += speech_s;
        let (e, w) = word_errors(&reference, &raw, lang);
        eprintln!(
            "{}: {:.1} s audio, {} segments, WER {:.1} %, RTF {:.3} ({:.1} s)",
            clip.id,
            pcm.len() as f64 / 16_000.0,
            segments,
            100.0 * e as f64 / w.max(1) as f64,
            decode_s / speech_s.max(1e-9),
            t0.elapsed().as_secs_f64()
        );
        clip_info.insert(
            clip.id.clone(),
            ClipInfo { category: clip.category.clone(), audio_s: pcm.len() as f64 / 16_000.0, segments },
        );
    }

    let baseline: BTreeMap<(String, String), f64> = match &args.baseline {
        Some(p) => {
            let b: BaselineFile = serde_json::from_str(&std::fs::read_to_string(p)?)
                .with_context(|| format!("parsing {}", p.display()))?;
            b.rows
                .into_iter()
                .filter(|r| r.decode == "greedy" && r.strategy == "only-sv")
                .filter_map(|r| r.wer.map(|w| ((r.post, r.category), w)))
                .collect()
        }
        None => BTreeMap::new(),
    };
    let ratio = |a: usize, b: usize| (b > 0).then(|| a as f64 / b as f64);
    let mut rows: Vec<Row> = acc
        .iter()
        .map(|((post, cat), a)| Row {
            post: post.clone(),
            category: cat.clone(),
            clips: a.clips,
            wer: ratio(a.errors, a.words),
            term_recall: ratio(a.term_found, a.term_total),
            term_exact: ratio(a.term_exact, a.term_total),
            rtf: (a.speech_s > 0.0).then(|| a.decode_s / a.speech_s),
            baseline_wer: baseline.get(&(post.clone(), cat.clone())).copied(),
        })
        .collect();
    rows.sort_by(|a, b| (a.category != "ALL", &a.category, &a.post).cmp(&(b.category != "ALL", &b.category, &b.post)));

    let summary = Summary {
        time: chrono::Local::now().to_rfc3339(),
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        threads: args.engine.threads,
        engine: rt.model_ids(),
        clips: clip_info,
        model_load_s: rt.load_time.as_secs_f64(),
        rss_mb_after_load: rss_after_load,
        rss_mb_peak: stats::peak_rss_mb(),
        engine_rtf: decode_total / speech_total.max(1e-9),
        rows,
        warnings,
    };
    let md = render(&summary);
    std::fs::write(out_dir.join("summary.json"), serde_json::to_string_pretty(&summary)?)?;
    std::fs::write(out_dir.join("summary.md"), &md)?;
    println!("{md}");
    eprintln!("results: {}", out_dir.display());
    Ok(())
}

/// Decodes a clip as one channel: (text, decode seconds, speech seconds, segments).
fn decode(rt: &Runtime, pcm: &[f32]) -> Result<(String, f64, f64, usize)> {
    let mut pipeline = rt.pipeline(Channel::Others, false)?;
    let mut texts = Vec::new();
    let (mut decode_s, mut speech_s) = (0.0, 0.0);
    let mut collect = |events: Vec<PipelineEvent>| {
        for e in events {
            if let PipelineEvent::Final { segment, stats, .. } = e {
                decode_s += stats.elapsed.as_secs_f64();
                speech_s += stats.audio.as_secs_f64();
                texts.push(segment.text);
            }
        }
    };
    for chunk in pcm.chunks(16_000) {
        collect(pipeline.push(chunk)?);
    }
    collect(pipeline.flush()?);
    Ok((texts.join(" "), decode_s, speech_s, texts.len()))
}

fn pct(x: Option<f64>) -> String {
    x.map_or("–".into(), |v| format!("{:.1}", 100.0 * v))
}

fn render(s: &Summary) -> String {
    let mut out = format!(
        "# tyst-cli bench · {}\n\n- Platform: `{}`, {} threads, engine {}\n- Clips: {}\n- Model load {:.1} s, RSS after load {:.0} MB, peak {:.0} MB\n- Decode RTF {:.3} (decode time / speech time)\n\n",
        s.time,
        s.platform,
        s.threads,
        s.engine.join(", "),
        s.clips.len(),
        s.model_load_s,
        s.rss_mb_after_load,
        s.rss_mb_peak,
        s.engine_rtf
    );
    out.push_str("WER, term recall and term exact spelling in %. Baseline: Phase 0 harness, greedy, only-sv.\n\n");
    out.push_str("| category | post | clips | WER | baseline WER | Δ pp | term recall | term exact | RTF |\n|---|---|---|---|---|---|---|---|---|\n");
    for r in &s.rows {
        let delta = match (r.wer, r.baseline_wer) {
            (Some(a), Some(b)) => format!("{:+.1}", 100.0 * (a - b)),
            _ => "–".into(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.category,
            r.post,
            r.clips,
            pct(r.wer),
            pct(r.baseline_wer),
            delta,
            pct(r.term_recall),
            pct(r.term_exact),
            r.rtf.map_or("–".into(), |v| format!("{v:.3}"))
        ));
    }
    if !s.warnings.is_empty() {
        out.push_str(&format!("\n{} manifest entries skipped (missing files).\n", s.warnings.len()));
    }
    out
}
