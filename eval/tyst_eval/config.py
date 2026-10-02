"""Loading of the model registry, the clip manifest and the vocabulary file."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

if sys.version_info >= (3, 11):
    import tomllib
else:  # pragma: no cover
    import tomli as tomllib

EVAL_DIR = Path(__file__).resolve().parent.parent

DEFAULT_MODELS_DIR = "~/.cache/tyst-eval/models"
DEFAULT_CLIPS_ROOT = "~/tyst-eval/clips"


def expand(path: str | Path) -> Path:
    return Path(os.path.expandvars(os.path.expanduser(str(path))))


def models_dir() -> Path:
    return expand(os.environ.get("TYST_MODELS", DEFAULT_MODELS_DIR))


# --------------------------------------------------------------------------- models


@dataclass
class ModelSpec:
    id: str
    kind: str  # "asr" | "vad" | "lid"
    dir: str
    lang: str = ""  # "sv" | "en" | "multi" (asr only)
    url: str = ""  # GitHub release archive
    sha256: str = ""  # of the archive or single file
    hf_repo: str = ""
    hf_revision: str = "main"
    hf_allow: list[str] = field(default_factory=list)
    files: dict[str, str] = field(default_factory=dict)  # optional explicit file names

    @property
    def path(self) -> Path:
        return models_dir() / self.dir


def load_models(path: Path | None = None) -> dict[str, ModelSpec]:
    path = path or EVAL_DIR / "models.toml"
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return {mid: ModelSpec(id=mid, **spec) for mid, spec in data["models"].items()}


# --------------------------------------------------------------------------- clips


@dataclass
class Clip:
    id: str
    category: str
    audio: Path
    reference: Path
    lang: str  # dominant language: "sv" | "en"
    lang_spans: list[tuple[float, float, str]] = field(default_factory=list)
    source: str = "owner"  # "owner" | "fleurs" | "synth"

    def lang_at(self, t: float) -> str:
        for start, end, lang in self.lang_spans:
            if start <= t < end:
                return lang
        return self.lang

    def exists(self) -> bool:
        return self.audio.is_file() and self.reference.is_file()


def clips_root(manifest: dict) -> Path:
    env = os.environ.get("TYST_EVAL_CLIPS")
    return expand(env or manifest.get("settings", {}).get("clips_root", DEFAULT_CLIPS_ROOT))


def _parse_clips(data: dict, base: Path, source: str) -> list[Clip]:
    clips = []
    for c in data.get("clip", []):
        clips.append(
            Clip(
                id=c["id"],
                category=c["category"],
                audio=base / expand(c["audio"]),
                reference=base / expand(c["reference"]),
                lang=c.get("lang", "sv"),
                lang_spans=[tuple(s) for s in c.get("lang_spans", [])],
                source=c.get("source", source),
            )
        )
    return clips


def load_manifest(path: Path | None = None) -> tuple[list[Clip], list[str], Path]:
    """Returns (clips, warnings, clips_root). Missing clips are reported, not fatal."""
    path = path or EVAL_DIR / "manifest.toml"
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    root = clips_root(data)
    clips = _parse_clips(data, root, "owner")
    warnings: list[str] = []
    for inc in data.get("include", []):
        inc_path = root / inc["path"]
        if not inc_path.is_file():
            warnings.append(f"include not found (skipped): {inc_path}")
            continue
        inc_data = tomllib.loads(inc_path.read_text(encoding="utf-8"))
        clips += _parse_clips(inc_data, inc_path.parent, inc.get("source", "owner"))
    present = []
    for c in clips:
        if c.exists():
            present.append(c)
        else:
            warnings.append(f"clip {c.id}: audio or reference missing (skipped)")
    return present, warnings, root


# --------------------------------------------------------------------------- vocabulary


@dataclass
class Vocabulary:
    terms: list[str]
    replacements: list[tuple[str, str]]


def load_vocabulary(path: Path | None = None) -> Vocabulary:
    path = path or EVAL_DIR / "terms.toml"
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return Vocabulary(
        terms=list(data.get("terms", [])),
        replacements=[(r["from"], r["to"]) for r in data.get("replacement", [])],
    )
