//! Text utilities shared by the pipeline and the benchmark: phrase matching, normalization, a
//! cheap sv/en guess, numeral spelling and word error rate. Ported from the Phase 0 harness
//! (`eval/tyst_eval/text.py`) so the CLI scores the same way.

use unicode_normalization::UnicodeNormalization;

use crate::asr::tokens::is_word_char;
use crate::transcript::Lang;

// --------------------------------------------------------------------------- phrase matching

/// A case-insensitive, whole-word phrase. Words in the phrase match across any run of
/// whitespace, like the harness's `(?<!\w)word\s+word(?!\w)` pattern.
#[derive(Debug, Clone)]
pub struct Phrase {
    words: Vec<Vec<char>>,
}

impl Phrase {
    pub fn new(phrase: &str) -> Self {
        Self { words: phrase.split_whitespace().map(|w| w.chars().flat_map(char::to_lowercase).collect()).collect() }
    }

    /// Byte ranges of non-overlapping matches, left to right.
    pub fn find_all(&self, text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if self.words.is_empty() {
            return out;
        }
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut i = 0;
        while i < chars.len() {
            let boundary_before = i == 0 || !is_word_char(chars[i - 1].1);
            if boundary_before
                && let Some(end) = self.match_at(&chars, i)
                && (end == chars.len() || !is_word_char(chars[end].1))
            {
                let b_end = chars.get(end).map_or(text.len(), |c| c.0);
                out.push((chars[i].0, b_end));
                i = end.max(i + 1);
                continue;
            }
            i += 1;
        }
        out
    }

    /// Char index just past a match starting at `i`.
    fn match_at(&self, chars: &[(usize, char)], mut i: usize) -> Option<usize> {
        for (w, word) in self.words.iter().enumerate() {
            if w > 0 {
                let ws_start = i;
                while i < chars.len() && chars[i].1.is_whitespace() {
                    i += 1;
                }
                if i == ws_start {
                    return None;
                }
            }
            let mut k = 0;
            while k < word.len() {
                let c = chars.get(i)?.1;
                for lc in c.to_lowercase() {
                    if word.get(k) != Some(&lc) {
                        return None;
                    }
                    k += 1;
                }
                i += 1;
            }
        }
        Some(i)
    }
}

/// Replaces every match of `phrase` with `with(matched_text, byte_start)`.
pub fn replace_phrase(text: &str, phrase: &Phrase, mut with: impl FnMut(&str, usize) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (a, b) in phrase.find_all(text) {
        out.push_str(&text[last..a]);
        out.push_str(&with(&text[a..b], a));
        last = b;
    }
    out.push_str(&text[last..]);
    out
}

// --------------------------------------------------------------------------- normalization

/// Lowercase, strip punctuation and collapse whitespace. Used for WER and term matching.
pub fn normalize(text: &str) -> String {
    let lowered: String = text.nfc().collect::<String>().to_lowercase().replace('\u{2019}', "'");
    // Punctuation (anything but word chars, whitespace, ' and -) and underscores become spaces.
    let stripped: Vec<char> = lowered
        .chars()
        .map(|c| if c == '_' || !(is_word_char(c) || c.is_whitespace() || c == '\'' || c == '-') { ' ' } else { c })
        .collect();
    // Hyphens and apostrophes only count inside words ("terraform-setup", "don't").
    let mut cleaned = String::with_capacity(stripped.len());
    for (i, &c) in stripped.iter().enumerate() {
        if c == '\'' || c == '-' {
            let before = i > 0 && is_word_char(stripped[i - 1]);
            let after = stripped.get(i + 1).is_some_and(|n| is_word_char(*n));
            cleaned.push(if before && after { c } else { ' ' });
        } else {
            cleaned.push(c);
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --------------------------------------------------------------------------- language guess

const SV_WORDS: &[&str] = &[
    "och", "att", "det", "som", "en", "är", "på", "för", "med", "har", "inte", "jag", "vi", "ni", "de", "den", "till",
    "av", "om", "så", "men", "kan", "var", "ska", "också", "eller", "när", "här", "där", "bara", "mycket", "nu", "då",
    "vad", "hur", "måste", "vår", "våra", "lite", "sedan", "efter", "kommer", "skulle", "finns", "blir", "eftersom",
    "alltså", "precis", "just", "ju", "väl", "hej", "tack", "bra",
];
const EN_WORDS: &[&str] = &[
    "the", "and", "to", "of", "a", "in", "is", "that", "it", "for", "you", "we", "on", "with", "this", "be", "are",
    "have", "not", "was", "but", "they", "at", "so", "if", "can", "will", "what", "just", "about", "from", "there",
    "would", "should", "our", "your", "do", "does", "going", "let's", "yeah", "okay", "hello", "thanks", "good",
];

/// Swedish, English, or `None` when the text carries no signal. Function-word counts plus a
/// half point per word with å, ä or ö.
pub fn guess_lang(text: &str) -> Option<Lang> {
    let norm = normalize(text);
    let words: Vec<&str> = norm.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let sv = words.iter().filter(|w| SV_WORDS.contains(w)).count() as f32
        + 0.5 * words.iter().filter(|w| w.contains(['å', 'ä', 'ö'])).count() as f32;
    let en = words.iter().filter(|w| EN_WORDS.contains(w)).count() as f32;
    if sv == en {
        None
    } else if sv > en {
        Some(Lang::Sv)
    } else {
        Some(Lang::En)
    }
}

// --------------------------------------------------------------------------- numerals

const SV_ONES: [&str; 20] = [
    "noll", "ett", "två", "tre", "fyra", "fem", "sex", "sju", "åtta", "nio", "tio", "elva", "tolv", "tretton",
    "fjorton", "femton", "sexton", "sjutton", "arton", "nitton",
];
const SV_TENS: [&str; 10] = ["", "", "tjugo", "trettio", "fyrtio", "femtio", "sextio", "sjuttio", "åttio", "nittio"];
const EN_ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const EN_TENS: [&str; 10] = ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];

/// Swedish writes numbers as one word: 21 -> tjugoett, 300 -> trehundra.
fn sv_words(n: u64) -> String {
    match n {
        0..=19 => SV_ONES[n as usize].to_string(),
        20..=99 => {
            format!(
                "{}{}",
                SV_TENS[(n / 10) as usize],
                if !n.is_multiple_of(10) { SV_ONES[(n % 10) as usize] } else { "" }
            )
        }
        100..=999 => format!(
            "{}hundra{}",
            if n >= 200 { SV_ONES[(n / 100) as usize] } else { "" },
            if !n.is_multiple_of(100) { sv_words(n % 100) } else { String::new() }
        ),
        _ => format!(
            "{}tusen{}",
            if n >= 2000 { sv_words(n / 1000) } else { String::new() },
            if !n.is_multiple_of(1000) { sv_words(n % 1000) } else { String::new() }
        ),
    }
}

fn en_words(n: u64) -> String {
    match n {
        0..=19 => EN_ONES[n as usize].to_string(),
        20..=99 => {
            let ones = if !n.is_multiple_of(10) { format!(" {}", EN_ONES[(n % 10) as usize]) } else { String::new() };
            format!("{}{}", EN_TENS[(n / 10) as usize], ones)
        }
        100..=999 => {
            let rest = if !n.is_multiple_of(100) { format!(" {}", en_words(n % 100)) } else { String::new() };
            format!("{} hundred{}", EN_ONES[(n / 100) as usize], rest)
        }
        _ => {
            let rest = if !n.is_multiple_of(1000) { format!(" {}", en_words(n % 1000)) } else { String::new() };
            format!("{} thousand{}", en_words(n / 1000), rest)
        }
    }
}

/// Integers 0-999999 written as digits become words, so "18" and "arton" score as equal.
/// Approximate on purpose, like the harness.
pub fn spell_numbers(text: &str, lang: Lang) -> String {
    let percent = match lang {
        Lang::Sv => " procent",
        Lang::En => " percent",
    };
    let chars: Vec<char> = text.chars().collect();
    let mut with_percent = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '%' && i > 0 && chars[i - 1].is_ascii_digit() {
            with_percent.push_str(percent);
        } else {
            with_percent.push(c);
        }
    }
    let chars: Vec<char> = with_percent.chars().collect();
    let mut out = String::with_capacity(with_percent.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !is_word_char(chars[i - 1])) {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let run: String = chars[i..j].iter().collect();
            let bounded = j == chars.len() || !is_word_char(chars[j]);
            match run.parse::<u64>() {
                Ok(n) if bounded && n < 1_000_000 => out.push_str(&match lang {
                    Lang::Sv => sv_words(n),
                    Lang::En => en_words(n),
                }),
                _ => out.push_str(&run),
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// --------------------------------------------------------------------------- metrics

/// (word edit distance, reference word count) on normalized text. With `lang`, digits are
/// spelled out first.
pub fn word_errors(reference: &str, hypothesis: &str, lang: Option<Lang>) -> (usize, usize) {
    let (r, h) = match lang {
        Some(l) => (normalize(&spell_numbers(reference, l)), normalize(&spell_numbers(hypothesis, l))),
        None => (normalize(reference), normalize(hypothesis)),
    };
    let r: Vec<&str> = r.split_whitespace().collect();
    let h: Vec<&str> = h.split_whitespace().collect();
    if r.is_empty() {
        return (h.len(), 0);
    }
    (edit_distance(&r, &h), r.len())
}

fn edit_distance(a: &[&str], b: &[&str]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(x != y);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Term accuracy counts per SPEC 12: (found case-insensitively, found with exact spelling,
/// occurrences in the reference).
pub fn term_hits(reference: &str, hypothesis: &str, terms: &[String]) -> (usize, usize, usize) {
    let (mut found, mut exact, mut total) = (0, 0, 0);
    for term in terms {
        let p = Phrase::new(term);
        let n_ref = p.find_all(reference).len();
        if n_ref == 0 {
            continue;
        }
        let hyp: Vec<&str> = p.find_all(hypothesis).into_iter().map(|(a, b)| &hypothesis[a..b]).collect();
        total += n_ref;
        found += n_ref.min(hyp.len());
        exact += n_ref.min(hyp.iter().filter(|m| *m == term).count());
    }
    (found, exact, total)
}

/// Term occurrences in `hypothesis` beyond those in `reference`: words the decoder put in that
/// were not said (decoy terms never occur in the reference, so every hit counts).
pub fn term_insertions(reference: &str, hypothesis: &str, terms: &[String]) -> usize {
    terms
        .iter()
        .map(|t| {
            let p = Phrase::new(t);
            p.find_all(hypothesis).len().saturating_sub(p.find_all(reference).len())
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_matches_whole_words_case_insensitively() {
        let p = Phrase::new("state drift");
        assert_eq!(p.find_all("Vår State   drift, och statedrift").len(), 1);
        assert!(Phrase::new("helm").find_all("helmet").is_empty());
        assert_eq!(Phrase::new("ÅTTA").find_all("åtta, Åtta").len(), 2);
    }

    #[test]
    fn replace_phrase_keeps_surroundings() {
        let out = replace_phrase("vi kör terra form nu", &Phrase::new("terra form"), |_, _| "Terraform".into());
        assert_eq!(out, "vi kör Terraform nu");
    }

    #[test]
    fn normalize_matches_harness() {
        assert_eq!(normalize("Hej, allihop! Vår Terraform-setup."), "hej allihop vår terraform-setup");
        assert_eq!(normalize("Don’t -- stop 'quoted' snake_case"), "don't stop quoted snake case");
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn spells_numbers_per_language() {
        assert_eq!(spell_numbers("18 st och 21 och 300", Lang::Sv), "arton st och tjugoett och trehundra");
        assert_eq!(spell_numbers("2026", Lang::Sv), "tvåtusentjugosex");
        assert_eq!(spell_numbers("1100", Lang::Sv), "tusenhundra");
        assert_eq!(spell_numbers("we have 42 and 115", Lang::En), "we have forty two and one hundred fifteen");
        assert_eq!(spell_numbers("50% 5G covid19", Lang::En), "fifty percent 5G covid19");
    }

    #[test]
    fn guesses_language_from_function_words() {
        assert_eq!(guess_lang("vi måste fixa vår Terraform state drift"), Some(Lang::Sv));
        assert_eq!(guess_lang("we should fix the state drift"), Some(Lang::En));
        assert_eq!(guess_lang("Terraform"), None);
    }

    #[test]
    fn counts_word_errors() {
        assert_eq!(word_errors("Hej på dig.", "hej på dig", None), (0, 3));
        assert_eq!(word_errors("a b c d", "a x c", None), (2, 4));
        assert_eq!(word_errors("arton grader", "18 grader", Some(Lang::Sv)), (0, 2));
        assert_eq!(word_errors("", "extra", None), (1, 0));
    }

    #[test]
    fn counts_term_hits() {
        let terms = vec!["Terraform".to_string(), "pull request".to_string()];
        let (found, exact, total) =
            term_hits("Terraform och Terraform, en pull request", "terraform och Terraform, en pull-request", &terms);
        assert_eq!((found, exact, total), (2, 1, 3));
    }

    #[test]
    fn counts_term_insertions() {
        let terms = vec!["Terraform".to_string(), "Grafana".to_string()];
        assert_eq!(term_insertions("Terraform igen", "terraform, Terraform och Grafana", &terms), 2);
        assert_eq!(term_insertions("Terraform och Terraform", "Terraform", &terms), 0);
    }
}
