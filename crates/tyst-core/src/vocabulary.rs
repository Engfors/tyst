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

/// Highest boost strength (alpha). Klang saw decoy words inserted and a worse WER at 2.0.
pub const MAX_BOOST_STRENGTH: f32 = 1.0;

/// The vocabulary file: same shape as `eval/terms.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabularyFile {
    /// Phrase boosting in the decoder.
    #[serde(default = "default_boost")]
    pub boost: bool,
    /// Boost weight, 0 to [`MAX_BOOST_STRENGTH`].
    #[serde(default = "default_boost_strength")]
    pub boost_strength: f32,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default, rename = "replacement")]
    pub replacements: Vec<Replacement>,
}

fn default_boost() -> bool {
    true
}

fn default_boost_strength() -> f32 {
    MAX_BOOST_STRENGTH
}

impl Default for VocabularyFile {
    fn default() -> Self {
        Self {
            boost: default_boost(),
            boost_strength: default_boost_strength(),
            terms: Vec::new(),
            replacements: Vec::new(),
        }
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
    /// Phrases to boost, and the strength; `None` when boosting is off.
    boost_phrases: Option<(Vec<String>, f32)>,
    /// The compiled boost, once a tokenizer is known ([`Self::with_boost`]).
    boost: Option<Arc<PhraseBoost>>,
}

impl VocabularyRules {
    pub fn new(file: &VocabularyFile) -> Self {
        let strength = file.boost_strength.clamp(0.0, MAX_BOOST_STRENGTH);
        let boost_phrases = (file.boost && strength > 0.0).then(|| {
            let terms = file.terms.iter().cloned();
            let tos = file.replacements.iter().filter(|r| r.boosts()).map(|r| r.to.clone());
            (terms.chain(tos).collect(), strength)
        });
        Self {
            replacements: file.replacements.iter().map(|r| (Phrase::new(&r.from), r.to.clone())).collect(),
            terms: file.terms.iter().map(|t| (Phrase::new(t), t.clone())).collect(),
            boost_phrases,
            boost: None,
        }
    }

    /// Compiles the boost with `encoder` (the decoder's tokenizer). Without one, or with nothing
    /// to boost, decoding stays unboosted.
    pub fn with_boost(mut self, encoder: Option<&SpmEncoder>) -> Self {
        self.boost = match (encoder, &self.boost_phrases) {
            (Some(encoder), Some((phrases, strength))) if !phrases.is_empty() => {
                let boost = PhraseBoost::new(phrases, encoder, *strength);
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
        assert_eq!(file.boost_strength, MAX_BOOST_STRENGTH);
        let r = VocabularyRules::new(&file);
        let (phrases, strength) = r.boost_phrases.clone().unwrap();
        assert_eq!(phrases, vec!["Terraform", "pull request", "HashiCorp", "Kubernetes"]);
        assert_eq!(strength, 1.0);
        // No tokenizer: nothing compiled.
        assert!(r.with_boost(None).boost().is_none());
    }

    #[test]
    fn boost_can_be_turned_off_and_strength_is_capped() {
        let off = VocabularyFile { boost: false, terms: vec!["Tyst".into()], ..Default::default() };
        assert!(VocabularyRules::new(&off).boost_phrases.is_none());
        let strong = VocabularyFile { boost_strength: 3.0, terms: vec!["Tyst".into()], ..Default::default() };
        assert_eq!(VocabularyRules::new(&strong).boost_phrases.unwrap().1, MAX_BOOST_STRENGTH);
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
        new.boost_strength = 0.5;
        new.replacements[0].boost = Some(true);
        let text = toml::to_string(&new).unwrap();
        assert_eq!(toml::from_str::<VocabularyFile>(&text).unwrap(), new);
        // An unset per-rule flag stays out of the file.
        let old_text = toml::to_string(&old).unwrap();
        assert!(!old_text.split("[[replacement]]").nth(1).unwrap().contains("boost"), "{old_text}");
    }

    #[test]
    fn joins_words_ignores_case_and_spacing() {
        assert!(Replacement::new("terra  form", "Terraform").joins_words());
        assert!(Replacement::new("klang ai", "Klang AI").joins_words());
        assert!(!Replacement::new("kuber netes", "kubectl").joins_words());
        assert!(!Replacement::new(" ", " ").joins_words());
    }
}
