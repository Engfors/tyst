"""Text normalization, vocabulary post-processing, a cheap sv/en text classifier and metrics."""

from __future__ import annotations

import re
import unicodedata

from .config import Vocabulary

_PUNCT = re.compile(r"[^\w\s'-]|_", re.UNICODE)
_SPACE = re.compile(r"\s+")


def normalize(text: str) -> str:
    """Lowercase, strip punctuation and collapse whitespace. Used for WER and term matching."""
    text = unicodedata.normalize("NFC", text).lower()
    text = text.replace("’", "'")
    text = _PUNCT.sub(" ", text)
    # hyphens/apostrophes only count inside words ("terraform-setup", "don't")
    text = re.sub(r"(?<!\w)['-]|['-](?!\w)", " ", text)
    return _SPACE.sub(" ", text).strip()


# --------------------------------------------------------------------------- vocabulary (SPEC 9.3)


def _ci_pattern(phrase: str) -> re.Pattern:
    words = [re.escape(w) for w in phrase.split()]
    return re.compile(r"(?<!\w)" + r"\s+".join(words) + r"(?!\w)", re.IGNORECASE)


def apply_vocabulary(text: str, vocab: Vocabulary) -> str:
    """Replacement rules, then canonical spelling of exact term matches. Whole-word, case-insensitive."""
    for src, dst in vocab.replacements:
        def repl(m: re.Match, dst: str = dst) -> str:
            at_sentence_start = m.start() == 0 or re.search(r"[.!?]\s*$", text[: m.start()]) is not None
            return dst[:1].upper() + dst[1:] if at_sentence_start else dst

        text = _ci_pattern(src).sub(repl, text)
    for term in vocab.terms:
        text = _ci_pattern(term).sub(term, text)
    return text


# --------------------------------------------------------------------------- language guess from text

_SV = set(
    "och att det som en är på för med har inte jag vi ni de den till av om så men kan var ska "
    "också eller när här där bara mycket nu då vad hur måste vår våra lite sedan efter kommer "
    "skulle finns blir eftersom alltså precis just ju väl hej tack bra".split()
)
_EN = set(
    "the and to of a in is that it for you we on with this be are have not was but they at "
    "so if can will what just about from there would should our your do does going let's yeah "
    "okay hello thanks good".split()
)


def guess_lang(text: str) -> str | None:
    """'sv', 'en' or None when the text carries no signal. Used by the Parakeet-first strategy."""
    words = normalize(text).split()
    if not words:
        return None
    sv = sum(w in _SV for w in words) + 0.5 * sum(any(c in w for c in "åäö") for w in words)
    en = sum(w in _EN for w in words)
    if sv == en:
        return None
    return "sv" if sv > en else "en"


# --------------------------------------------------------------------------- metrics


def word_errors(ref: str, hyp: str) -> tuple[int, int]:
    """(edit distance in words, reference word count) on normalized text."""
    import jiwer

    r, h = normalize(ref), normalize(hyp)
    if not r:
        return len(h.split()), 0
    out = jiwer.process_words(r, h if h else "")
    return out.substitutions + out.deletions + out.insertions, len(r.split())


def term_hits(ref: str, hyp: str, terms: list[str]) -> tuple[int, int, int]:
    """(found case-insensitive, found with exact spelling, occurrences in reference) per SPEC 12."""
    found = exact = total = 0
    for term in terms:
        pat = _ci_pattern(term)
        n_ref = len(pat.findall(ref))
        if not n_ref:
            continue
        hyp_matches = pat.findall(hyp)
        total += n_ref
        found += min(n_ref, len(hyp_matches))
        exact += min(n_ref, sum(m == term for m in hyp_matches))
    return found, exact, total
