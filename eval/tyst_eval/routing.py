"""Language routing strategies (SPEC 6.3), evaluated offline over pre-decoded segments.

Every segment is decoded once by each engine (and run through LID once); a strategy only decides
which of those results it would have used and what that would have cost. That keeps the strategies
comparable and the run cheap, and the decision logic here is what Phase 1 ports to Rust.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .engines import AsrResult
from .text import guess_lang

SV_LIKE = {"sv", "no", "nn", "da"}  # Whisper LID often confuses Scandinavian languages


@dataclass
class SegmentData:
    start: float
    end: float
    truth: str  # "sv" | "en"
    decoded: dict[str, AsrResult]  # role ("sv" | "en") -> result, for one decode config
    lid: str | None = None  # raw LID label
    lid_seconds: float = 0.0

    @property
    def duration(self) -> float:
        return self.end - self.start


@dataclass
class RouterParams:
    initial_lang: str = "sv"  # meeting language prior
    short_segment: float = 1.5  # shorter segments inherit the current language
    dual_bias: float = 0.05  # log-prob bonus for the current language (dual-decode)


@dataclass
class Routed:
    lang: str
    text: str
    seconds: float  # decode + LID cost this strategy would have paid


@dataclass
class Strategy:
    name: str
    needs: set[str] = field(default_factory=set)  # roles/"lid" required


def lid_to_lang(label: str | None) -> str | None:
    if label is None:
        return None
    if label in SV_LIKE:
        return "sv"
    if label == "en":
        return "en"
    return None


def route(strategy: str, segments: list[SegmentData], p: RouterParams) -> list[Routed]:
    current = p.initial_lang
    out: list[Routed] = []
    for seg in segments:
        sv, en = seg.decoded.get("sv"), seg.decoded.get("en")
        short = seg.duration < p.short_segment

        if strategy == "only-sv":
            out.append(Routed("sv", sv.text, sv.seconds))
            continue
        if strategy == "only-en":
            out.append(Routed("en", en.text, en.seconds))
            continue

        if short:
            r = seg.decoded[current]
            out.append(Routed(current, r.text, r.seconds))
            continue

        if strategy == "lid":
            lang = lid_to_lang(seg.lid) or current
            r = seg.decoded[lang]
            out.append(Routed(lang, r.text, seg.lid_seconds + r.seconds))
        elif strategy == "dual":
            s_sv = sv.confidence + (p.dual_bias if current == "sv" else 0.0)
            s_en = en.confidence + (p.dual_bias if current == "en" else 0.0)
            lang = "sv" if s_sv >= s_en else "en"
            out.append(Routed(lang, seg.decoded[lang].text, sv.seconds + en.seconds))
        elif strategy == "parakeet-first":
            lang = guess_lang(en.text) or current
            if lang == "sv":
                out.append(Routed("sv", sv.text, en.seconds + sv.seconds))
            else:
                out.append(Routed("en", en.text, en.seconds))
        else:
            raise ValueError(f"unknown strategy {strategy}")
        current = out[-1].lang
    return out


STRATEGIES = ["only-sv", "only-en", "lid", "dual", "parakeet-first"]
