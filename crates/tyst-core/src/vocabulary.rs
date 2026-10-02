//! Custom vocabulary (SPEC 9.3): replacement rules for systematic misrecognitions, then canonical
//! spelling of known terms. Both match whole words, case-insensitively.
//!
//! Phrase boosting in the decoder is not used in v1 (ADR 0001), so these rules carry term spelling.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::text::{Phrase, replace_phrase};
use crate::{Error, Result};

/// The vocabulary file: same shape as `eval/terms.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VocabularyFile {
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default, rename = "replacement")]
    pub replacements: Vec<Replacement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

/// Compiled vocabulary rules.
#[derive(Debug, Clone, Default)]
pub struct VocabularyRules {
    replacements: Vec<(Phrase, String)>,
    terms: Vec<(Phrase, String)>,
}

impl VocabularyRules {
    pub fn new(file: &VocabularyFile) -> Self {
        Self {
            replacements: file.replacements.iter().map(|r| (Phrase::new(&r.from), r.to.clone())).collect(),
            terms: file.terms.iter().map(|t| (Phrase::new(t), t.clone())).collect(),
        }
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
        let file = VocabularyFile {
            terms: vec![],
            replacements: vec![Replacement { from: "kuber netes".into(), to: "kubectl".into() }],
        };
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
}
