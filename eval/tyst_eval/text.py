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


# --------------------------------------------------------------------------- numerals

_SV_ONES = "noll ett två tre fyra fem sex sju åtta nio tio elva tolv tretton fjorton femton sexton sjutton arton nitton".split()
_SV_TENS = "_ _ tjugo trettio fyrtio femtio sextio sjuttio åttio nittio".split()
_EN_ONES = ("zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen "
            "sixteen seventeen eighteen nineteen").split()
_EN_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()


def _sv_words(n: int) -> str:
    """Swedish writes numbers as one word: 21 -> tjugoett, 300 -> trehundra."""
    if n < 20:
        return _SV_ONES[n]
    if n < 100:
        return _SV_TENS[n // 10] + (_SV_ONES[n % 10] if n % 10 else "")
    if n < 1000:
        return (_SV_ONES[n // 100] if n >= 200 else "") + "hundra" + (_sv_words(n % 100) if n % 100 else "")
    return (_sv_words(n // 1000) if n >= 2000 else "") + "tusen" + (_sv_words(n % 1000) if n % 1000 else "")


def _en_words(n: int) -> str:
    if n < 20:
        return _EN_ONES[n]
    if n < 100:
        return _EN_TENS[n // 10] + (" " + _EN_ONES[n % 10] if n % 10 else "")
    if n < 1000:
        return _EN_ONES[n // 100] + " hundred" + (" " + _en_words(n % 100) if n % 100 else "")
    return _en_words(n // 1000) + " thousand" + (" " + _en_words(n % 1000) if n % 1000 else "")


def spell_numbers(text: str, lang: str) -> str:
    """Integers 0-999999 written as digits -> words, so "18" and "arton" score as equal.

    Approximate on purpose: "ett"/"en", "hundra"/"etthundra" and year readings still differ.
    """
    words = _sv_words if lang == "sv" else _en_words

    def repl(m: re.Match) -> str:
        n = int(m.group(0))
        return words(n) if n < 1_000_000 else m.group(0)

    text = re.sub(r"(?<=\d)%", " procent" if lang == "sv" else " percent", text)
    return re.sub(r"\b\d+\b", repl, text)


# --------------------------------------------------------------------------- metrics


def word_errors(ref: str, hyp: str, lang: str | None = None) -> tuple[int, int]:
    """(edit distance in words, reference word count) on normalized text.

    With `lang`, digits are spelled out first (see spell_numbers).
    """
    import jiwer

    if lang:
        ref, hyp = spell_numbers(ref, lang), spell_numbers(hyp, lang)
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
