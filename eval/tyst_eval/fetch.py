"""Downloads: model files (`fetch-models`) and public FLEURS samples (`fetch-fleurs`).

These are the only network calls in the harness. Audio is written below the clips root, never
into the repository.
"""

from __future__ import annotations

import io
import random
import shutil
import tarfile
import urllib.request
from pathlib import Path

import numpy as np

from .config import ModelSpec, load_manifest, load_models, models_dir
from .pins import app_pins, check_archive, check_snapshot


def _download(url: str, dest: Path) -> None:
    tmp = dest.with_suffix(dest.suffix + ".part")
    print(f"downloading {url}")
    with urllib.request.urlopen(url) as resp, tmp.open("wb") as out:
        shutil.copyfileobj(resp, out, 1 << 20)
    tmp.rename(dest)


def fetch_model(spec: ModelSpec) -> None:
    target = spec.path
    if target.is_dir() and any(target.iterdir()):
        print(f"{spec.id}: present at {target}")
        return
    root = models_dir()
    root.mkdir(parents=True, exist_ok=True)
    if spec.url:
        name = spec.url.rsplit("/", 1)[-1]
        dest = root / name
        if not dest.exists():
            _download(spec.url, dest)
        problem = check_archive(dest, spec.sha256)
        if problem:
            dest.unlink()
            raise SystemExit(f"{spec.id}: {name}: {problem}")
        print(f"{spec.id}: sha256 verified")
        if name.endswith((".tar.bz2", ".tar.gz")):
            with tarfile.open(dest) as tar:
                tar.extractall(root, filter="data")
            dest.unlink()
        else:
            target.mkdir(parents=True, exist_ok=True)
            dest.rename(target / name)
    elif spec.hf_repo:
        try:
            from huggingface_hub import snapshot_download
        except ImportError as e:
            raise SystemExit("pip install huggingface_hub to fetch Hugging Face models") from e
        path = snapshot_download(spec.hf_repo, revision=spec.hf_revision, allow_patterns=spec.hf_allow or None, local_dir=target)
        problems = check_snapshot(Path(path), app_pins(spec.hf_repo, spec.hf_revision))
        if problems:
            raise SystemExit(f"{spec.id}: " + "; ".join(problems))
        print(f"{spec.id}: {spec.hf_repo}@{spec.hf_revision} -> {path}, checksums verified")
    else:
        raise SystemExit(f"{spec.id}: no download source in models.toml")


def fetch_models(args) -> None:
    models = load_models()
    for mid in args.ids or list(models):
        try:
            fetch_model(models[mid])
        except Exception as e:  # noqa: BLE001 - keep going with the other models
            print(f"{mid}: FAILED: {e}")


# --------------------------------------------------------------------------- FLEURS

FLEURS_LANGS = {"sv": "sv_se", "en": "en_us"}


def _write_wav(path: Path, pcm: np.ndarray) -> None:
    import soundfile as sf

    sf.write(str(path), pcm, 16000, subtype="PCM_16")


def fetch_fleurs(args) -> None:
    """Pick `n` test utterances per language from google/fleurs and build mixed-language concatenations."""
    import soundfile as sf
    from huggingface_hub import hf_hub_download
    from scipy.signal import resample_poly

    _, _, root = load_manifest()
    out = root / "fleurs"
    out.mkdir(parents=True, exist_ok=True)
    entries, utts = [], {"sv": [], "en": []}
    for lang, code in FLEURS_LANGS.items():
        tsv = Path(hf_hub_download("google/fleurs", f"data/{code}/test.tsv", repo_type="dataset"))
        audio_tar = Path(hf_hub_download("google/fleurs", f"data/{code}/audio/test.tar.gz", repo_type="dataset"))
        rows, seen = [], set()
        for line in tsv.read_text(encoding="utf-8").splitlines():
            cols = line.split("\t")
            if len(cols) < 3 or cols[0] in seen:
                continue
            seen.add(cols[0])
            rows.append((cols[1], cols[2]))  # file name, raw transcription
        random.Random(0).shuffle(rows)
        rows = dict(rows[: args.n])
        with tarfile.open(audio_tar) as tar:
            for member in tar:
                name = Path(member.name).name
                if name not in rows:
                    continue
                pcm, sr = sf.read(io.BytesIO(tar.extractfile(member).read()), dtype="float32")
                if pcm.ndim > 1:
                    pcm = pcm.mean(axis=1)
                if sr != 16000:
                    pcm = resample_poly(pcm, 16000, sr).astype(np.float32)
                cid = f"fleurs-{lang}-{Path(name).stem}"
                _write_wav(out / f"{cid}.wav", pcm)
                (out / f"{cid}.txt").write_text(rows[name] + "\n", encoding="utf-8")
                entries.append((cid, f"fleurs-{lang}", lang, []))
                utts[lang].append((pcm, rows[name]))
    # Mixed clips: sv block, en block, sv block (pattern A in SPEC 6.3), 0.8 s silence between utterances
    gap = np.zeros(int(0.8 * 16000), dtype=np.float32)
    for k in range(args.mixed):
        plan = [("sv", 2), ("en", 2), ("sv", 1)]
        pcm_parts, text_parts, spans, t = [], [], [], 0.0
        for lang, count in plan:
            start = t
            for _ in range(count):
                pcm, text = utts[lang][(k * 3 + len(text_parts)) % len(utts[lang])]
                pcm_parts += [pcm, gap]
                text_parts.append(text)
                t += (len(pcm) + len(gap)) / 16000
            spans.append([round(start, 2), round(t, 2), lang])
        cid = f"fleurs-mixed-{k + 1:02d}"
        _write_wav(out / f"{cid}.wav", np.concatenate(pcm_parts))
        (out / f"{cid}.txt").write_text(" ".join(text_parts) + "\n", encoding="utf-8")
        entries.append((cid, "fleurs-mixed", "sv", spans))
    write_manifest(out / "manifest.toml", entries, "fleurs", "Generated by `python -m tyst_eval fetch-fleurs` from google/fleurs (CC BY 4.0).")
    print(f"wrote {len(entries)} FLEURS clips to {out}")


def write_manifest(path: Path, entries: list[tuple[str, str, str, list]], source: str, comment: str) -> None:
    lines = [f"# {comment}", ""]
    for cid, cat, lang, spans in entries:
        lines += ["[[clip]]", f'id = "{cid}"', f'category = "{cat}"', f'audio = "{cid}.wav"', f'reference = "{cid}.txt"',
                  f'lang = "{lang}"', f'source = "{source}"']
        if spans:
            lines.append("lang_spans = [" + ", ".join(f'[{a}, {b}, "{l}"]' for a, b, l in spans) + "]")
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")
