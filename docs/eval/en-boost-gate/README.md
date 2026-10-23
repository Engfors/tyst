# English phrase-boost gate (issue #38, option 2)

Committed, shareable `summary.json` / `summary.md` only (no hypotheses).

| Run | Clips | Notes |
|---|---|---|
| [`fleurs-en/`](fleurs-en/) | 25 FLEURS English | Term-free; `eval/terms-en.toml` (names + decoys); `--lang en --boost 0.25,0.5,1` |

Owner `en-01`…`en-05` wavs are not in git. Re-run [`eval/english-boost-gate.sh`](../../eval/english-boost-gate.sh) when they are under `$TYST_EVAL_CLIPS/owner/`.

Decision: product `BOOST_ENGLISH = false` because strength **1.0** WER on `fleurs-en` was **26.1 %** vs raw **25.7 %** (0 term insertions).
