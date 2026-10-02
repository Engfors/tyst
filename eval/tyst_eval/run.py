"""`python -m tyst_eval run`: decode the manifest with every engine and score every strategy."""

from __future__ import annotations

import json
import platform
import time
from collections import defaultdict
from dataclasses import asdict
from pathlib import Path

from . import __version__
from .config import EVAL_DIR, Clip, load_manifest, load_models, load_vocabulary, models_dir
from .engines import AsrEngine, DecodeConfig, SegmentationParams, WhisperLid, load_audio, vad_segments
from .perf import peak_rss_mb
from .routing import STRATEGIES, RouterParams, SegmentData, lid_to_lang, route
from .text import apply_vocabulary, term_hits, word_errors


def _env_info(args) -> dict:
    import sherpa_onnx

    return {
        "harness": __version__,
        "time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "python": platform.python_version(),
        "sherpa_onnx": sherpa_onnx.__version__,
        "threads": args.threads,
        "provider": args.provider,
        "models_dir": str(models_dir()),
    }


def run(args) -> Path:
    models = load_models()
    vocab = load_vocabulary()
    clips, warnings, root = load_manifest(Path(args.manifest) if args.manifest else None)
    if args.categories:
        wanted = set(args.categories.split(","))
        clips = [c for c in clips if c.category in wanted]
    if args.limit:
        clips = clips[: args.limit]
    for w in warnings:
        print(f"warning: {w}")
    if not clips:
        raise SystemExit(f"no clips found (clips root: {root})")

    roles = dict(kv.split("=") for kv in args.engines.split(","))
    decodes = args.decode.split(",")
    seg_params = SegmentationParams(min_silence=args.min_silence, max_speech=args.max_speech)
    router = RouterParams(initial_lang=args.initial_lang, short_segment=args.short_segment, dual_bias=args.dual_bias)

    out_dir = Path(args.out) if args.out else EVAL_DIR / "results" / time.strftime("%Y%m%d-%H%M%S")
    out_dir.mkdir(parents=True, exist_ok=True)
    work = out_dir / "work"

    # ---- load engines (one instance per model id and decode config)
    engines: dict[tuple[str, str], AsrEngine] = {}
    load_info = {}
    for decode in decodes:
        cfg = DecodeConfig(name=decode, hotwords=vocab.terms, hotwords_score=args.hotwords_score)
        for model_id in sorted(set(roles.values())):
            try:
                eng = AsrEngine(models[model_id], cfg, args.threads, args.provider, work)
            except Exception as e:  # noqa: BLE001 - surfaced in the report
                print(f"error: cannot load {model_id} ({decode}): {e}")
                load_info[f"{model_id}/{decode}"] = {"error": str(e)}
                continue
            engines[(model_id, decode)] = eng
            load_info[eng.id] = {"backend": eng.backend, "load_seconds": round(eng.load_seconds, 2)}
            print(f"loaded {eng.id} via {eng.backend} in {eng.load_seconds:.1f}s")
    lid = WhisperLid(models[args.lid], args.threads, args.provider) if args.lid else None
    vad_model = models["silero-vad"].path / "silero_vad.onnx"
    rss_after_load = peak_rss_mb()

    # ---- decode
    seg_log = (out_dir / "segments.jsonl").open("w", encoding="utf-8")
    per_clip: dict[str, dict] = {}
    engine_time = defaultdict(float)
    engine_audio = defaultdict(float)
    lid_stats = defaultdict(lambda: [0, 0])  # truth -> [correct, total]
    for clip in clips:
        pcm = load_audio(clip.audio)
        segs = vad_segments(pcm, vad_model, seg_params)
        ref = clip.reference.read_text(encoding="utf-8")
        print(f"{clip.id}: {len(pcm) / 16000:.1f}s audio, {len(segs)} segments")
        by_decode: dict[str, list[SegmentData]] = {d: [] for d in decodes}
        for i, s in enumerate(segs):
            truth = clip.lang_at((s.start + s.end) / 2)
            lid_label, lid_s = lid.detect(s.samples) if lid else (None, 0.0)
            if lid:
                lid_stats[truth][1] += 1
                lid_stats[truth][0] += lid_to_lang(lid_label) == truth
            for decode in decodes:
                cache = {}
                for model_id in set(roles.values()):
                    eng = engines.get((model_id, decode))
                    if eng:
                        cache[model_id] = eng.transcribe(s.samples)
                        engine_time[eng.id] += cache[model_id].seconds
                        engine_audio[eng.id] += s.duration
                decoded = {role: cache[m] for role, m in roles.items() if m in cache}
                by_decode[decode].append(SegmentData(s.start, s.end, truth, decoded, lid_label, lid_s))
                seg_log.write(json.dumps({
                    "clip": clip.id, "seg": i, "start": round(s.start, 2), "end": round(s.end, 2),
                    "truth": truth, "lid": lid_label, "decode": decode,
                    **{f"{role}_text": r.text for role, r in decoded.items()},
                    **{f"{role}_conf": round(r.confidence, 4) for role, r in decoded.items()},
                }, ensure_ascii=False) + "\n")
        per_clip[clip.id] = {"clip": clip, "ref": ref, "audio_s": len(pcm) / 16000, "segments": by_decode}
    seg_log.close()

    # ---- score
    rows = []
    hyp_dir = out_dir / "hyp"
    hyp_dir.mkdir(exist_ok=True)
    for decode in decodes:
        for strategy in STRATEGIES:
            for post in ("raw", "vocab"):
                acc = defaultdict(lambda: defaultdict(float))
                for cid, info in per_clip.items():
                    clip: Clip = info["clip"]
                    segs = info["segments"][decode]
                    needed = {"only-sv": ("sv",), "only-en": ("en",)}.get(strategy, ("sv", "en"))
                    if any(role not in s.decoded for s in segs for role in needed):
                        continue
                    if strategy == "lid" and not lid:
                        continue
                    routed = route(strategy, segs, router)
                    hyp = " ".join(r.text for r in routed)
                    if post == "vocab":
                        hyp = apply_vocabulary(hyp, vocab)
                    if post == "raw":
                        (hyp_dir / f"{cid}.{decode}.{strategy}.txt").write_text(hyp + "\n", encoding="utf-8")
                    errs, n = word_errors(info["ref"], hyp, None if args.no_number_norm else clip.lang)
                    found, exact, total = term_hits(info["ref"], hyp, vocab.terms)
                    wrong = sum(r.lang != s.truth for r, s in zip(routed, segs))
                    for key in (clip.category, "ALL"):
                        a = acc[key]
                        a["errs"] += errs; a["words"] += n
                        a["t_found"] += found; a["t_exact"] += exact; a["t_total"] += total
                        a["wrong"] += wrong; a["segs"] += len(segs)
                        a["cost"] += sum(r.seconds for r in routed); a["audio"] += sum(s.duration for s in segs)
                        a["clips"] += 1
                for cat, a in acc.items():
                    rows.append({
                        "decode": decode, "strategy": strategy, "post": post, "category": cat,
                        "clips": int(a["clips"]),
                        "wer": a["errs"] / a["words"] if a["words"] else None,
                        "term_recall": a["t_found"] / a["t_total"] if a["t_total"] else None,
                        "term_exact": a["t_exact"] / a["t_total"] if a["t_total"] else None,
                        "wrong_engine": a["wrong"] / a["segs"] if a["segs"] else None,
                        "rtf": a["cost"] / a["audio"] if a["audio"] else None,
                    })

    summary = {
        "env": _env_info(args),
        "roles": roles,
        "decodes": decodes,
        "lid": args.lid,
        "segmentation": asdict(seg_params),
        "router": asdict(router),
        "clips": {cid: {"category": i["clip"].category, "source": i["clip"].source, "audio_s": round(i["audio_s"], 1)} for cid, i in per_clip.items()},
        "load": load_info,
        "engine_rtf": {eid: engine_time[eid] / engine_audio[eid] for eid in engine_time if engine_audio[eid]},
        "lid_accuracy": {t: c / n for t, (c, n) in lid_stats.items() if n},
        "rss_mb_after_load": rss_after_load,
        "rss_mb_peak": peak_rss_mb(),
        "rows": rows,
        "warnings": warnings,
    }
    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8")
    (out_dir / "summary.md").write_text(render_markdown(summary), encoding="utf-8")
    print(render_markdown(summary))
    print(f"\nresults: {out_dir}")
    return out_dir


def _pct(x) -> str:
    return "–" if x is None else f"{100 * x:.1f}"


def _rtf(x) -> str:
    return "–" if x is None else f"{x:.3f}"


def render_markdown(s: dict) -> str:
    lines = [
        f"# Phase 0 run · {s['env']['time']}",
        "",
        f"- Platform: `{s['env']['platform']}` ({s['env']['machine']}), provider `{s['env']['provider']}`, {s['env']['threads']} threads",
        f"- sherpa-onnx {s['env']['sherpa_onnx']}, harness {s['env']['harness']}",
        f"- Engines: {', '.join(f'{r} = `{m}`' for r, m in s['roles'].items())}; LID: `{s['lid']}`",
        f"- Clips: {len(s['clips'])} ({', '.join(sorted({c['category'] for c in s['clips'].values()}))})",
        f"- Peak RSS: {s['rss_mb_peak']:.0f} MB (after model load: {s['rss_mb_after_load']:.0f} MB)",
        "",
        "## Engines",
        "",
        "| engine | backend | load s | RTF |",
        "|---|---|---|---|",
    ]
    for eid, info in s["load"].items():
        if "error" in info:
            lines.append(f"| {eid} | load failed: {info['error'][:80]} | | |")
        else:
            lines.append(f"| {eid} | {info['backend']} | {info['load_seconds']} | {s['engine_rtf'].get(eid, 0):.3f} |")
    if s["lid_accuracy"]:
        lines += ["", "## LID accuracy (per segment, sv-like labels count as sv)", ""]
        lines += [f"- {t}: {_pct(a)} %" for t, a in sorted(s["lid_accuracy"].items())]
    lines += [
        "",
        "## Strategies",
        "",
        "WER, term recall and term exact-spelling are in %, wrong-engine rate is the share of segments routed to the other language's engine, RTF is decode + LID time / speech time.",
        "",
        "| decode | strategy | post | category | clips | WER | term recall | term exact | wrong engine | RTF |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for r in sorted(s["rows"], key=lambda r: (r["category"] != "ALL", r["category"], r["decode"], r["strategy"], r["post"])):
        lines.append(
            f"| {r['decode']} | {r['strategy']} | {r['post']} | {r['category']} | {r['clips']} | {_pct(r['wer'])} | "
            f"{_pct(r['term_recall'])} | {_pct(r['term_exact'])} | {_pct(r['wrong_engine'])} | "
            f"{_rtf(r['rtf'])} |"
        )
    if s["warnings"]:
        lines += ["", f"{len(s['warnings'])} manifest entries skipped (missing files)."]
    return "\n".join(lines) + "\n"
