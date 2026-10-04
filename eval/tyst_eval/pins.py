"""Checksums for downloaded models: the same SHA-256 pins as the app's `models/models.toml`.

No model is used unverified: a URL download needs a `sha256` in `eval/models.toml`, and a
Hugging Face snapshot must match the app manifest's pins for its repo and revision.
"""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

if sys.version_info >= (3, 11):
    import tomllib
else:  # pragma: no cover
    import tomli as tomllib

APP_MANIFEST = Path(__file__).resolve().parent.parent.parent / "models" / "models.toml"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def app_pins(hf_repo: str, revision: str, manifest: Path = APP_MANIFEST) -> dict[str, str]:
    """File name -> SHA-256 pinned in the app manifest for this repo at this revision."""
    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    for spec in data.get("models", {}).values():
        if spec.get("hf_repo") == hf_repo and spec.get("revision") == revision:
            return {f["name"]: f["sha256"] for f in spec.get("files", [])}
    return {}


def check_snapshot(path: Path, pins: dict[str, str]) -> list[str]:
    """Problems with a downloaded snapshot: an unpinned or mismatching model file, or a pinned
    file that is missing. Empty when everything matches."""
    if not pins:
        return ["no SHA-256 pins for this repo and revision in models/models.toml"]
    problems = []
    for name, sha in sorted(pins.items()):
        f = path / name
        if not f.is_file():
            problems.append(f"{name}: missing")
        elif sha256_file(f) != sha:
            problems.append(f"{name}: checksum mismatch")
    for f in sorted(path.rglob("*.onnx")):
        if f.name not in pins:
            problems.append(f"{f.name}: not pinned")
    return problems


def check_archive(path: Path, sha256: str | None) -> str | None:
    """Problem with a URL download, or None. A missing pin is a problem, not a skip."""
    if not sha256:
        return "no sha256 pinned in eval/models.toml"
    digest = sha256_file(path)
    if digest != sha256:
        return f"checksum mismatch: {digest} != {sha256}"
    return None
