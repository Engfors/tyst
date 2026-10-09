//! Custom vocabulary (SPEC 9.3): replacement rules for systematic misrecognitions, then canonical
//! spelling of known terms. Both match whole words, case-insensitively.
//!
//! The same lists also bias the decoder (phrase boosting, `asr::boost`): every term, and the `to`
//! side of replacements that only join words (`terra form` -> `Terraform`), or of any rule marked
//! with `boost = true`. The `from` side is never boosted, since it is the misrecognition. The text
//! rules still run afterwards, so they catch what boosting misses and fix casing.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::asr::boost::PhraseBoost;
use crate::asr::spm::SpmEncoder;
use crate::text::{Phrase, replace_phrase};
use crate::{Error, Result};

/// The boost strength (alpha) the app decodes with. Fixed, not a setting. On the owner's `sv-terms`
/// clips (`tyst-cli bench --boost 0.25,0.5,1`), 1.0 had the best term recall and WER; `sv` was unchanged.
pub const BOOST_STRENGTH: f32 = 1.0;
/// Highest strength `bench --boost` accepts. Klang saw decoy words inserted and a worse WER at 2.0.
pub const MAX_BOOST_STRENGTH: f32 = 1.0;
/// Longest term or replacement side, in characters. Encoding a phrase is quadratic in its length.
pub const MAX_PHRASE_CHARS: usize = 200;
/// Most terms, and most replacements, in one vocabulary.
pub const MAX_ENTRIES: usize = 2000;

/// The vocabulary file: same shape as `eval/terms.toml`. Unknown keys are ignored, so a
/// `boost_strength` written by a test build still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabularyFile {
    /// Phrase boosting in the decoder.
    #[serde(default = "default_boost")]
    pub boost: bool,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default, rename = "replacement")]
    pub replacements: Vec<Replacement>,
}

fn default_boost() -> bool {
    true
}

impl Default for VocabularyFile {
    fn default() -> Self {
        Self { boost: default_boost(), terms: Vec::new(), replacements: Vec::new() }
    }
}

impl VocabularyFile {
    /// Rejects entries over [`MAX_PHRASE_CHARS`] and lists over [`MAX_ENTRIES`], for files the
    /// user saves or imports. Rules built from a file that slipped past skip such phrases.
    pub fn check(&self) -> Result<()> {
        let too_long = |s: &str| s.chars().count() > MAX_PHRASE_CHARS;
        if self.terms.len() > MAX_ENTRIES || self.replacements.len() > MAX_ENTRIES {
            return Err(Error::Config(format!(
                "vocabulary: at most {MAX_ENTRIES} terms and {MAX_ENTRIES} replacements"
            )));
        }
        let long = self.terms.iter().any(|t| too_long(t))
            || self.replacements.iter().any(|r| too_long(&r.from) || too_long(&r.to));
        if long {
            return Err(Error::Config(format!("vocabulary: entries are limited to {MAX_PHRASE_CHARS} characters")));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
    /// Boost `to` in the decoder. Unset: only when the rule just joins words ([`Self::joins_words`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost: Option<bool>,
}

impl Replacement {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), to: to.into(), boost: None }
    }

    /// True when `to` is `from` with the spaces taken out, ignoring case (`terra form` ->
    /// `Terraform`): then `to` is what was said, spelled as one unit, and worth boosting.
    pub fn joins_words(&self) -> bool {
        let squeeze =
            |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
        let to = squeeze(&self.to);
        !to.is_empty() && squeeze(&self.from) == to
    }

    pub fn boosts(&self) -> bool {
        self.boost.unwrap_or_else(|| self.joins_words())
    }
}

/// Compiled vocabulary rules.
#[derive(Debug, Clone, Default)]
pub struct VocabularyRules {
    replacements: Vec<(Phrase, String)>,
    terms: Vec<(Phrase, String)>,
    /// Phrases to boost; `None` when boosting is off.
    boost_phrases: Option<Vec<String>>,
    /// The compiled boost, once a tokenizer is known ([`Self::with_boost`]).
    boost: Option<Arc<PhraseBoost>>,
    /// Also boost Parakeet (forced English); only `bench` sets it, until an English eval passes.
    boost_english: bool,
}

impl VocabularyRules {
    pub fn new(file: &VocabularyFile) -> Self {
        let boost_phrases = file.boost.then(|| {
            let terms = file.terms.iter().take(MAX_ENTRIES);
            let tos = file.replacements.iter().take(MAX_ENTRIES).filter(|r| r.boosts()).map(|r| &r.to);
            let fits = |p: &&String| p.chars().count() <= MAX_PHRASE_CHARS;
            terms.chain(tos).filter(fits).cloned().collect()
        });
        Self {
            replacements: file.replacements.iter().map(|r| (Phrase::new(&r.from), r.to.clone())).collect(),
            terms: file.terms.iter().map(|t| (Phrase::new(t), t.clone())).collect(),
            boost_phrases,
            boost: None,
            boost_english: false,
        }
    }

    /// Compiles the boost with `encoder` (the decoder's tokenizer) at [`BOOST_STRENGTH`]. Without
    /// an encoder, or with nothing to boost, decoding stays unboosted.
    pub fn with_boost(self, encoder: Option<&SpmEncoder>) -> Self {
        self.with_boost_at(encoder, BOOST_STRENGTH)
    }

    /// [`with_boost`](Self::with_boost) at another strength, capped at [`MAX_BOOST_STRENGTH`]
    /// (for `bench --boost`).
    pub fn with_boost_at(mut self, encoder: Option<&SpmEncoder>, strength: f32) -> Self {
        let strength = strength.min(MAX_BOOST_STRENGTH);
        self.boost = match (encoder, &self.boost_phrases) {
            (Some(encoder), Some(phrases)) if !phrases.is_empty() && strength > 0.0 => {
                let boost = PhraseBoost::new(phrases, encoder, strength);
                log::info!("phrase boost: {} phrases, {} states, strength {strength}", phrases.len(), boost.tree.len());
                Some(Arc::new(boost))
            }
            _ => None,
        };
        self
    }

    #[cfg(test)]
    pub(crate) fn with_phrase_boost(mut self, boost: PhraseBoost) -> Self {
        self.boost = Some(Arc::new(boost));
        self
    }

    /// Only the boost, without the text rules: for scoring the decoder's output on its own.
    pub fn boost_only(mut self) -> Self {
        self.replacements.clear();
        self.terms.clear();
        self
    }

    /// The compiled boost, if any.
    pub fn boost(&self) -> Option<&PhraseBoost> {
        self.boost.as_deref()
    }

    /// Boosts Parakeet (forced English) too, for `bench --lang en --boost`. Parakeet v3 shares
    /// Pianissimo's tokenizer (identical `vocab.txt`), so the same tree applies.
    pub fn with_english_boost(mut self) -> Self {
        self.boost_english = true;
        self
    }

    /// True when the boost also applies to Parakeet.
    pub fn boosts_english(&self) -> bool {
        self.boost_english
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self::new(&load_file(path)?))
    }

    pub fn is_empty(&self) -> bool {
        self.replacements.is_empty() && self.terms.is_empty()
    }

    /// Applies replacements (keeping a capital at sentence start), then term spelling.
    pub fn apply(&self, text: &str) -> String {
        let mut text = text.to_string();
        for (phrase, to) in &self.replacements {
            let before = text.clone();
            text = replace_phrase(&before, phrase, |_, start| {
                if at_sentence_start(&before[..start]) { capitalize(to) } else { to.clone() }
            });
        }
        for (phrase, term) in &self.terms {
            text = replace_phrase(&text, phrase, |_, _| term.clone());
        }
        text
    }
}

pub fn load_file(path: &Path) -> Result<VocabularyFile> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
}

/// True at the start of the text or after `.`, `!` or `?` plus optional whitespace.
fn at_sentence_start(prefix: &str) -> bool {
    match prefix.trim_end().chars().last() {
        None => true,
        Some(c) => matches!(c, '.' | '!' | '?'),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> VocabularyRules {
        let file: VocabularyFile = toml::from_str(
            r#"
            terms = ["Terraform", "HashiCorp", "pull request", "Kubernetes"]
            [[replacement]]
            from = "terra form"
            to = "terraform"
            [[replacement]]
            from = "hashi corp"
            to = "HashiCorp"
            "#,
        )
        .unwrap();
        VocabularyRules::new(&file)
    }

    #[test]
    fn replaces_then_spells_terms() {
        let r = rules();
        assert_eq!(r.apply("vi kör terra form och hashi corp vault"), "vi kör Terraform och HashiCorp vault");
        assert_eq!(r.apply("en PULL  REQUEST till kubernetes"), "en pull request till Kubernetes");
    }

    #[test]
    fn capitalizes_replacements_at_sentence_start() {
        let file =
            VocabularyFile { replacements: vec![Replacement::new("kuber netes", "kubectl")], ..Default::default() };
        let r = VocabularyRules::new(&file);
        assert_eq!(
            r.apply("kuber netes först. kuber netes igen, och kuber netes"),
            "Kubectl först. Kubectl igen, och kubectl"
        );
    }

    #[test]
    fn leaves_partial_words_alone() {
        assert_eq!(rules().apply("terraformering"), "terraformering");
    }

    #[test]
    fn loads_the_phase0_terms_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/terms.toml");
        let file = load_file(&path).unwrap();
        assert!(file.terms.contains(&"Terraform".to_string()));
        assert!(!file.replacements.is_empty());
    }

    #[test]
    fn boosts_terms_and_joined_replacements_only() {
        let file: VocabularyFile = toml::from_str(
            r#"
            terms = ["Terraform", "pull request"]
            [[replacement]]
            from = "hashi corp"
            to = "HashiCorp"
            [[replacement]]
            from = "kuber netes"
            to = "kubectl"
            [[replacement]]
            from = "kubernetis"
            to = "Kubernetes"
            boost = true
            [[replacement]]
            from = "git hub"
            to = "GitHub"
            boost = false
            "#,
        )
        .unwrap();
        assert!(file.boost);
        let r = VocabularyRules::new(&file);
        assert_eq!(r.boost_phrases.clone().unwrap(), vec!["Terraform", "pull request", "HashiCorp", "Kubernetes"]);
        // No tokenizer: nothing compiled.
        assert!(r.with_boost(None).boost().is_none());
    }

    #[test]
    fn boost_can_be_turned_off() {
        let off = VocabularyFile { boost: false, terms: vec!["Tyst".into()], ..Default::default() };
        assert!(VocabularyRules::new(&off).boost_phrases.is_none());
    }

    #[test]
    fn long_entries_and_lists_are_limited() {
        let long = "x".repeat(MAX_PHRASE_CHARS + 1);
        let file = VocabularyFile { terms: vec!["Tyst".into(), long.clone()], ..Default::default() };
        assert!(file.check().is_err());
        assert_eq!(VocabularyRules::new(&file).boost_phrases.unwrap(), vec!["Tyst"]);
        let rule = VocabularyFile { replacements: vec![Replacement::new("a", &long)], ..Default::default() };
        assert!(rule.check().is_err());
        let many = VocabularyFile { terms: vec!["a".into(); MAX_ENTRIES + 1], ..Default::default() };
        assert!(many.check().is_err());
        assert_eq!(VocabularyRules::new(&many).boost_phrases.unwrap().len(), MAX_ENTRIES);
        let fine = VocabularyFile { terms: vec!["å".repeat(MAX_PHRASE_CHARS)], ..Default::default() };
        assert!(fine.check().is_ok());
    }

    #[test]
    fn old_files_load_and_new_fields_round_trip() {
        let old: VocabularyFile =
            toml::from_str("terms = [\"a\"]\n[[replacement]]\nfrom = \"b\"\nto = \"c\"\n").unwrap();
        assert_eq!(
            old,
            VocabularyFile {
                terms: vec!["a".into()],
                replacements: vec![Replacement::new("b", "c")],
                ..Default::default()
            }
        );
        let mut new = old.clone();
        new.boost = false;
        new.replacements[0].boost = Some(true);
        let text = toml::to_string(&new).unwrap();
        assert_eq!(toml::from_str::<VocabularyFile>(&text).unwrap(), new);
        // An unset per-rule flag stays out of the file.
        let old_text = toml::to_string(&old).unwrap();
        assert!(!old_text.split("[[replacement]]").nth(1).unwrap().contains("boost"), "{old_text}");
        // A strength saved by a test build is ignored.
        let test_build: VocabularyFile = toml::from_str("boost_strength = 0.5\nterms = [\"a\"]\n").unwrap();
        assert_eq!(test_build.terms, vec!["a"]);
    }

    #[test]
    fn joins_words_ignores_case_and_spacing() {
        assert!(Replacement::new("terra  form", "Terraform").joins_words());
        assert!(Replacement::new("klang ai", "Klang AI").joins_words());
        assert!(!Replacement::new("kuber netes", "kubectl").joins_words());
        assert!(!Replacement::new(" ", " ").joins_words());
    }
}
