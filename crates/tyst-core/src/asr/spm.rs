//! SentencePiece BPE encoder (text -> token IDs), a pure-Rust port of the parts of sentencepiece
//! v0.2.0 that Pianissimo's and Parakeet v3's `tokenizer.model` use: the precompiled `nmt_nfkc`
//! normalizer, user-defined symbols and score-ordered BPE merges. Only phrase boosting needs it;
//! decoding goes through `vocab.txt` (`tokens`).

use std::collections::HashMap;
use std::path::Path;

use protobuf::Message;

use crate::{Error, Result};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/spm_proto/mod.rs"));
}

use generated::sentencepiece_model::ModelProto;
use generated::sentencepiece_model::model_proto::sentence_piece::Type as PieceType;
use generated::sentencepiece_model::trainer_spec::ModelType;

const SPACE: &str = "\u{2581}";

#[derive(Debug, Clone)]
pub struct SpmEncoder {
    /// Piece -> (id, score) for every piece BPE may produce.
    pieces: HashMap<String, (u32, f32)>,
    /// Never normalized or merged (in these models: digits and `<|...|>` tags).
    user_defined: Vec<String>,
    unk: u32,
    size: usize,
    charsmap: Option<CharsMap>,
    add_dummy_prefix: bool,
    remove_extra_whitespaces: bool,
    escape_whitespaces: bool,
}

impl SpmEncoder {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let model = ModelProto::parse_from_bytes(bytes).map_err(|e| Error::Model(format!("tokenizer: {e}")))?;
        let trainer = &model.trainer_spec;
        if trainer.model_type() != ModelType::BPE {
            return Err(Error::Model("tokenizer: only BPE models are supported".into()));
        }
        if trainer.byte_fallback() || trainer.treat_whitespace_as_suffix() {
            return Err(Error::Model("tokenizer: byte fallback and suffix whitespace are not supported".into()));
        }
        let mut pieces = HashMap::new();
        let mut user_defined = Vec::new();
        for (id, p) in model.pieces.iter().enumerate() {
            match p.type_() {
                PieceType::NORMAL | PieceType::UNUSED => {}
                PieceType::USER_DEFINED => user_defined.push(p.piece().to_string()),
                PieceType::UNKNOWN | PieceType::CONTROL => continue,
                PieceType::BYTE => return Err(Error::Model("tokenizer: byte pieces are not supported".into())),
            }
            pieces.insert(p.piece().to_string(), (id as u32, p.score()));
        }
        // Longest match first, as sentencepiece's prefix matcher does.
        user_defined.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let unk = trainer.unk_id();
        if unk < 0 || unk as usize >= model.pieces.len() {
            return Err(Error::Model("tokenizer: unk id out of range".into()));
        }
        let norm = &model.normalizer_spec;
        let charsmap = match norm.precompiled_charsmap() {
            [] => None,
            blob => Some(CharsMap::parse(blob)?),
        };
        Ok(Self {
            pieces,
            user_defined,
            unk: unk as u32,
            size: model.pieces.len(),
            charsmap,
            add_dummy_prefix: norm.add_dummy_prefix(),
            remove_extra_whitespaces: norm.remove_extra_whitespaces(),
            escape_whitespaces: norm.escape_whitespaces(),
        })
    }

    /// Pieces in the model, `<unk>` and control pieces included: the decoder's vocabulary
    /// without blank.
    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Token IDs for `text`, exactly as `SentencePieceProcessor.encode` returns them.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let normalized = self.normalize(text);
        let mut symbols = self.split(&normalized);
        // Merge the adjacent pair whose union scores highest (leftmost on ties) until none is a piece.
        // Quadratic, which is fine for phrases; sentencepiece uses a heap for the same order.
        loop {
            let mut best: Option<(usize, f32)> = None;
            for i in 0..symbols.len().saturating_sub(1) {
                let (l, r) = (&symbols[i], &symbols[i + 1]);
                if l.frozen || r.frozen {
                    continue;
                }
                let merged = &normalized[l.start..r.end];
                if let Some(&(_, score)) = self.pieces.get(merged)
                    && best.is_none_or(|(_, s)| score > s)
                {
                    best = Some((i, score));
                }
            }
            let Some((i, _)) = best else { break };
            symbols[i].end = symbols[i + 1].end;
            symbols.remove(i + 1);
        }
        let mut ids = Vec::with_capacity(symbols.len());
        for s in &symbols {
            let id = self.pieces.get(&normalized[s.start..s.end]).map_or(self.unk, |&(id, _)| id);
            // Runs of unknown pieces come out as one, like sentencepiece's output.
            if id == self.unk && ids.last() == Some(&self.unk) {
                continue;
            }
            ids.push(id);
        }
        ids
    }

    fn user_defined_at(&self, s: &str) -> Option<usize> {
        self.user_defined.iter().find(|u| s.starts_with(u.as_str())).map(String::len)
    }

    /// sentencepiece's `Normalizer::Normalize`, without the alignment bookkeeping.
    fn normalize(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + SPACE.len());
        if text.is_empty() {
            return out;
        }
        let mut input = text;
        if self.remove_extra_whitespaces {
            input = input.trim_start_matches(' ');
        }
        let space = if self.escape_whitespaces { SPACE } else { " " };
        if self.add_dummy_prefix {
            out.push_str(space);
        }
        let mut prev_space = self.remove_extra_whitespaces;
        while !input.is_empty() {
            let (mut piece, consumed) = self.normalize_prefix(input);
            if prev_space {
                piece = piece.trim_start_matches(' ');
            }
            if !piece.is_empty() {
                for c in piece.chars() {
                    if c == ' ' { out.push_str(space) } else { out.push(c) }
                }
                prev_space = piece.ends_with(' ');
            }
            input = &input[consumed..];
            if !self.remove_extra_whitespaces {
                prev_space = false;
            }
        }
        if self.remove_extra_whitespaces {
            while out.ends_with(space) {
                out.truncate(out.len() - space.len());
            }
        }
        out
    }

    /// Normalized text for the longest normalizable prefix of `input`, and its length in bytes.
    fn normalize_prefix<'a>(&'a self, input: &'a str) -> (&'a str, usize) {
        if let Some(n) = self.user_defined_at(input) {
            return (&input[..n], n);
        }
        if let Some((len, normalized)) = self.charsmap.as_ref().and_then(|m| m.longest_prefix(input.as_bytes())) {
            // Matches end on a character boundary: the trie holds whole UTF-8 sequences.
            return (normalized, len);
        }
        let n = input.chars().next().map_or(0, char::len_utf8);
        (&input[..n], n)
    }

    /// Initial symbols: user-defined pieces whole (frozen), everything else one character each.
    fn split(&self, s: &str) -> Vec<Symbol> {
        let mut symbols = Vec::new();
        let mut i = 0;
        while i < s.len() {
            let (len, frozen) = match self.user_defined_at(&s[i..]) {
                Some(n) => (n, true),
                None => (s[i..].chars().next().map_or(1, char::len_utf8), false),
            };
            symbols.push(Symbol { start: i, end: i + len, frozen });
            i += len;
        }
        symbols
    }
}

#[derive(Debug, Clone, Copy)]
struct Symbol {
    start: usize,
    end: usize,
    frozen: bool,
}

/// `precompiled_charsmap`: a little-endian `u32` trie size, a darts-clone double-array trie over
/// UTF-8 bytes, then NUL-terminated replacement strings that the trie values point into.
#[derive(Debug, Clone)]
struct CharsMap {
    units: Vec<u32>,
    normalized: String,
    /// Byte offsets of the NUL terminators in `normalized`, to slice out each replacement.
    ends: Vec<usize>,
}

impl CharsMap {
    fn parse(blob: &[u8]) -> Result<Self> {
        let bad = || Error::Model("tokenizer: malformed precompiled charsmap".into());
        let size = u32::from_le_bytes(blob.get(..4).ok_or_else(bad)?.try_into().map_err(|_| bad())?) as usize;
        if size == 0 || !size.is_multiple_of(4) {
            return Err(bad());
        }
        let trie = blob.get(4..4 + size).ok_or_else(bad)?;
        let units = trie.as_chunks::<4>().0.iter().map(|&c| u32::from_le_bytes(c)).collect();
        let normalized = String::from_utf8(blob[4 + size..].to_vec()).map_err(|_| bad())?;
        let ends = normalized.match_indices('\0').map(|(i, _)| i).collect();
        Ok(Self { units, normalized, ends })
    }

    /// darts-clone `commonPrefixSearch`, keeping the longest hit.
    fn longest_prefix(&self, key: &[u8]) -> Option<(usize, &str)> {
        let unit = |pos: usize| self.units.get(pos).copied();
        let offset = |u: u32| ((u >> 10) << ((u & (1 << 9)) >> 6)) as usize;
        let mut pos = offset(unit(0)?);
        let mut best = None;
        for (i, &b) in key.iter().enumerate() {
            pos ^= b as usize;
            let u = unit(pos)?;
            if u & (0x8000_0000 | 0xFF) != b as u32 {
                break;
            }
            pos ^= offset(u);
            if (u >> 8) & 1 == 1 {
                let value = (unit(pos)? & 0x7FFF_FFFF) as usize;
                best = Some((i + 1, value));
            }
        }
        let (len, start) = best?;
        let end = *self.ends.get(self.ends.partition_point(|&e| e < start))?;
        Some((len, &self.normalized[start..end]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use generated::sentencepiece_model::model_proto::SentencePiece;
    use generated::sentencepiece_model::{NormalizerSpec, TrainerSpec};

    /// A tiny BPE model: `<unk>`, digits as user-defined pieces, then merges by descending score.
    fn model(normal: &[&str], remove_extra_whitespaces: bool) -> SpmEncoder {
        let mut m = ModelProto::new();
        let mut push = |piece: &str, score: f32, ty: PieceType| {
            let mut p = SentencePiece::new();
            p.set_piece(piece.into());
            p.set_score(score);
            p.set_type(ty);
            m.pieces.push(p);
        };
        push("<unk>", 0.0, PieceType::UNKNOWN);
        push("1", 0.0, PieceType::USER_DEFINED);
        push("12", 0.0, PieceType::USER_DEFINED);
        for (i, p) in normal.iter().enumerate() {
            push(p, -(i as f32), PieceType::NORMAL);
        }
        let mut t = TrainerSpec::new();
        t.set_model_type(ModelType::BPE);
        m.trainer_spec = Some(t).into();
        let mut n = NormalizerSpec::new();
        n.set_remove_extra_whitespaces(remove_extra_whitespaces);
        m.normalizer_spec = Some(n).into();
        SpmEncoder::parse(&m.write_to_bytes().unwrap()).unwrap()
    }

    const PIECES: &[&str] = &["▁", "t", "e", "r", "a", "f", "o", "m", "er", "▁t", "▁te", "ra", "▁ter", "or", "▁terra"];

    #[test]
    fn merges_by_score() {
        let m = model(PIECES, false);
        // "▁t" outranks "er" and "ra"; then "▁te", "▁ter", "ra", "▁terra"... ids are offset by 3.
        assert_eq!(m.encode("terra"), vec![3 + 14]);
        // "form" has no "fo"/"rm" pieces, only "or".
        assert_eq!(m.encode("terraform"), vec![3 + 14, 3 + 5, 3 + 13, 3 + 7]);
    }

    #[test]
    fn spaces_become_word_starts_and_are_kept() {
        let m = model(PIECES, false);
        assert_eq!(m.encode("a  t"), vec![3, 3 + 4, 3, 3 + 9]);
        assert_eq!(m.encode(""), Vec::<u32>::new());
        let squeezed = model(PIECES, true);
        assert_eq!(squeezed.encode("  a  t "), vec![3, 3 + 4, 3 + 9]);
    }

    #[test]
    fn user_defined_pieces_are_frozen_and_unknowns_collapse() {
        let m = model(PIECES, false);
        // "12" wins over "1" (longest match); "3" is unknown, so are "x" and "y" (one id).
        assert_eq!(m.encode("t12"), vec![3 + 9, 2]);
        assert_eq!(m.encode("t13xy"), vec![3 + 9, 1, 0]);
    }

    #[test]
    fn rejects_unigram_models() {
        let mut m = ModelProto::new();
        let mut t = TrainerSpec::new();
        t.set_model_type(ModelType::UNIGRAM);
        m.trainer_spec = Some(t).into();
        assert!(SpmEncoder::parse(&m.write_to_bytes().unwrap()).is_err());
        assert!(SpmEncoder::parse(b"not a model").is_err());
    }
}
