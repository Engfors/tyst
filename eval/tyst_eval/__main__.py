"""CLI entry point: `python -m tyst_eval <command>` (run from the eval/ directory)."""

from __future__ import annotations

import argparse
import json
import os


def main() -> None:
    cpu = os.cpu_count() or 4
    p = argparse.ArgumentParser(prog="tyst_eval", description="Tyst Phase 0 evaluation harness")
    sub = p.add_subparsers(dest="cmd", required=True)

    f = sub.add_parser("fetch-models", help="download models listed in models.toml")
    f.add_argument("ids", nargs="*")

    ff = sub.add_parser("fetch-fleurs", help="download FLEURS sv/en test samples and build mixed clips")
    ff.add_argument("--n", type=int, default=25, help="utterances per language")
    ff.add_argument("--mixed", type=int, default=5, help="mixed-language clips to build")

    s = sub.add_parser("synth", help="generate synthetic smoke-test clips with Piper TTS")
    s.add_argument("--n", type=int, default=5, help="clips per category")

    r = sub.add_parser("run", help="decode the manifest and score all strategies")
    r.add_argument("--manifest")
    r.add_argument("--engines", default="sv=pianissimo-sv-int8,en=parakeet-v3-int8", help="role=model-id pairs")
    r.add_argument("--lid", default="whisper-tiny", help="LID model id, empty to disable")
    r.add_argument("--decode", default="greedy,hotwords", help="greedy, beam, hotwords (comma separated)")
    r.add_argument("--hotwords-score", type=float, default=1.5)
    r.add_argument("--threads", type=int, default=min(4, cpu))
    r.add_argument("--provider", default="cpu", choices=["cpu", "coreml", "cuda"])
    r.add_argument("--categories", help="only these categories (comma separated)")
    r.add_argument("--limit", type=int)
    r.add_argument("--initial-lang", default="sv")
    r.add_argument("--short-segment", type=float, default=1.5)
    r.add_argument("--dual-bias", type=float, default=0.05)
    r.add_argument("--min-silence", type=float, default=0.5)
    r.add_argument("--max-speech", type=float, default=15.0)
    r.add_argument("--no-number-norm", action="store_true", help="score digits and spelled-out numbers as different words")
    r.add_argument("--out")

    b = sub.add_parser("bench", help="RTF and RSS per model, each in a fresh process")
    b.add_argument("--models", default="pianissimo-sv-int8,parakeet-v3-int8")
    b.add_argument("--combined", default="whisper-tiny", help="extra models for the all-loaded-together run")
    b.add_argument("--threads-list", default=f"1,{min(4, cpu)}")
    b.add_argument("--provider", default="cpu", choices=["cpu", "coreml", "cuda"])
    b.add_argument("--seconds", type=float, default=120)
    b.add_argument("--chunk", type=float, default=8, help="segment length fed to the model")
    b.add_argument("--out")

    bo = sub.add_parser("bench-one")  # internal, used by bench
    bo.add_argument("--models", required=True)
    bo.add_argument("--threads", type=int, required=True)
    bo.add_argument("--provider", default="cpu")
    bo.add_argument("--seconds", type=float, default=120)
    bo.add_argument("--chunk", type=float, default=8)

    args = p.parse_args()
    if args.cmd == "fetch-models":
        from .fetch import fetch_models

        fetch_models(args)
    elif args.cmd == "fetch-fleurs":
        from .fetch import fetch_fleurs

        fetch_fleurs(args)
    elif args.cmd == "synth":
        from .synth import synth

        synth(args)
    elif args.cmd == "run":
        from .run import run

        run(args)
    elif args.cmd == "bench":
        from .perf import bench

        bench(args)
    elif args.cmd == "bench-one":
        from .perf import bench_one

        print(json.dumps(bench_one(args.models.split(","), args.threads, args.provider, args.seconds, args.chunk)))


if __name__ == "__main__":
    main()
