//! SentencePiece vocabulary (`vocab.txt`: one `<piece> <id>` per line) and detokenizing.

use std::path::Path;

use crate::{Error, Result};

const WORD_START: char = '\u{2581}';

#[derive(Debug, Clone)]
pub struct Vocabulary {
    pieces: Vec<String>,
    blank: u32,
}

impl Vocabulary {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut entries = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let (piece, id) = line
                .rsplit_once(' ')
                .ok_or_else(|| Error::Model(format!("vocab line {}: expected '<piece> <id>'", n + 1)))?;
            let id: usize = id.trim().parse().map_err(|_| Error::Model(format!("vocab line {}: bad id", n + 1)))?;
            entries.push((id, piece.to_string()));
        }
        let mut pieces = vec![String::new(); entries.len()];
        for (id, piece) in entries {
            if id >= pieces.len() {
                return Err(Error::Model(format!("vocab id {id} out of range")));
            }
            pieces[id] = piece;
        }
        let blank =
            pieces.iter().position(|p| p == "<blk>").ok_or_else(|| Error::Model("vocab has no <blk> token".into()))?
                as u32;
        Ok(Self { pieces, blank })
    }

    /// Token count, blank included.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    pub fn blank(&self) -> u32 {
        self.blank
    }

    pub fn piece(&self, id: u32) -> &str {
        &self.pieces[id as usize]
    }

    /// Joins pieces into text the way onnx-asr does: a word-start marker becomes a space before a
    /// word character and disappears elsewhere (text start, before punctuation).
    pub fn detokenize(&self, ids: &[u32]) -> String {
        let joined: String = ids.iter().map(|&id| self.piece(id)).collect();
        let chars: Vec<char> = joined.chars().collect();
        let mut out = String::with_capacity(joined.len());
        for (i, &c) in chars.iter().enumerate() {
            if c != WORD_START {
                out.push(c);
                continue;
            }
            let next_is_word = chars.get(i + 1).is_some_and(|n| is_word_char(*n));
            if i > 0 && next_is_word {
                out.push(' ');
            }
        }
        out.trim().to_string()
    }
}

/// Python's `\w`: letters, digits and underscore.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocabulary {
        Vocabulary::parse("<unk> 0\n▁hej 1\n▁på 2\n▁dig 3\n, 4\n▁ 5\n. 6\nform 7\n▁Terra 8\n<blk> 9\n").unwrap()
    }

    #[test]
    fn parses_and_finds_blank() {
        let v = vocab();
        assert_eq!(v.len(), 10);
        assert_eq!(v.blank(), 9);
        assert_eq!(v.piece(2), "▁på");
    }

    #[test]
    fn detokenizes_words_and_punctuation() {
        let v = vocab();
        assert_eq!(v.detokenize(&[1, 4, 2, 3, 6]), "hej, på dig.");
        assert_eq!(v.detokenize(&[8, 7]), "Terraform");
        // A bare marker before punctuation vanishes.
        assert_eq!(v.detokenize(&[1, 5, 6]), "hej.");
        assert_eq!(v.detokenize(&[]), "");
    }

    #[test]
    fn rejects_vocab_without_blank() {
        assert!(Vocabulary::parse("a 0\nb 1\n").is_err());
    }
}
