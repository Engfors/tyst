"""Memory and speed measurements. `bench` runs each configuration in a fresh process so RSS is isolated."""

from __future__ import annotations

import json
import platform
import resource
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

from .config import EVAL_DIR, load_manifest, load_models, models_dir


def peak_rss_mb() -> float:
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # Linux reports KiB, macOS bytes
    return peak / (1024 * 1024) if platform.system() == "Darwin" else peak / 1024


def current_rss_mb() -> float:
    import psutil

    return psutil.Process().memory_info().rss / (1024 * 1024)


def bench_audio(seconds: float) -> np.ndarray:
    """Speech from the manifest (first clips until `seconds` is reached), so timing reflects real audio."""
    from .engines import load_audio

    clips, _, root = load_manifest()
    parts, total = [], 0
    for c in clips:
        pcm = load_audio(c.audio)
        parts.append(pcm)
        total += len(pcm)
        if total >= seconds * 16000:
            break
    if not parts:
        raise SystemExit(f"bench needs at least one clip in the manifest (clips root: {root})")
    return np.concatenate(parts)[: int(seconds * 16000)]


def bench_one(model_ids: list[str], threads: int, provider: str, seconds: float, chunk: float) -> dict:
    """Load the given models together, then decode `seconds` of speech in `chunk`-second pieces with each ASR model."""
    from .engines import AsrEngine, DecodeConfig, WhisperLid

    models = load_models()
    rss0 = current_rss_mb()
    audio = bench_audio(seconds)
    loaded, out = {}, {"models": model_ids, "threads": threads, "provider": provider, "audio_s": len(audio) / 16000}
    for mid in model_ids:
        spec = models[mid]
        t0 = time.perf_counter()
        if spec.kind == "asr":
            loaded[mid] = AsrEngine(spec, DecodeConfig(), threads, provider, EVAL_DIR / "results" / "work")
        elif spec.kind == "lid":
            loaded[mid] = WhisperLid(spec, threads, provider)
        out[f"load_s/{mid}"] = round(time.perf_counter() - t0, 2)
    out["rss_mb_after_load"] = round(current_rss_mb() - rss0, 0)
    step = int(chunk * 16000)
    for mid, eng in loaded.items():
        t0 = time.perf_counter()
        for i in range(0, len(audio), step):
            piece = audio[i : i + step]
            if len(piece) < 8000:
                continue
            eng.transcribe(piece) if hasattr(eng, "transcribe") else eng.detect(piece)
        out[f"rtf/{mid}"] = round((time.perf_counter() - t0) / (len(audio) / 16000), 4)
    out["rss_mb_peak"] = round(peak_rss_mb(), 0)
    return out


def bench(args) -> None:
    models = load_models()
    asr = [m for m in args.models.split(",") if m]
    configs = [[m] for m in asr]
    if args.combined:
        configs.append(asr + [m for m in args.combined.split(",") if m])
    results = []
    for threads in [int(t) for t in args.threads_list.split(",")]:
        for cfg in configs:
            missing = [m for m in cfg if m not in models or not models[m].path.is_dir()]
            if missing:
                print(f"skip {cfg}: not downloaded: {missing} (models dir {models_dir()})")
                continue
            cmd = [sys.executable, "-m", "tyst_eval", "bench-one", "--models", ",".join(cfg), "--threads", str(threads),
                   "--provider", args.provider, "--seconds", str(args.seconds), "--chunk", str(args.chunk)]
            proc = subprocess.run(cmd, cwd=EVAL_DIR, capture_output=True, text=True)
            if proc.returncode != 0:
                print(f"bench {cfg} @ {threads} threads failed:\n{proc.stderr[-2000:]}")
                continue
            res = json.loads(proc.stdout.strip().splitlines()[-1])
            results.append(res)
            print(json.dumps(res))
    out = Path(args.out) if args.out else EVAL_DIR / "results" / f"bench-{time.strftime('%Y%m%d-%H%M%S')}.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"platform": platform.platform(), "machine": platform.machine(), "results": results}, indent=2))
    print(f"\nresults: {out}")
