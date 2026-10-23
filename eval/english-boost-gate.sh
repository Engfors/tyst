#!/usr/bin/env bash
# Issue #38 option 2: English (Parakeet) phrase-boosting eval matrix.
# Needs: `tyst-cli` on PATH or at TARGET, models (`tyst-cli models fetch`), and audio.
#
# Owner clips are NOT in git. Put en-01.wav…en-05.wav (+ matching .txt refs) under
# $TYST_EVAL_CLIPS/owner/ (default ~/tyst-eval/clips/owner/). Optional: copy
# eval/owner-clips/references/en-*.txt next to them.
#
# FLEURS: `cd eval && python3 -m tyst_eval fetch-fleurs` (writes under clips root).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLI="${TYST_CLI:-$ROOT/target/release/tyst-cli}"
MANIFEST="${MANIFEST:-$ROOT/eval/manifest.toml}"
TERMS_EN="${TERMS_EN:-$ROOT/eval/terms-en.toml}"
OUT_ROOT="${OUT_ROOT:-$ROOT/eval/results/en-boost-gate}"
BOOST="${BOOST:-0.25,0.5,1}"
THREADS="${THREADS:-4}"

if [[ ! -x "$CLI" ]]; then
  echo "error: tyst-cli not found at $CLI (build with: cargo build -p tyst-cli --release)" >&2
  exit 1
fi

run_one() {
  local name="$1"
  shift
  local out="$OUT_ROOT/$name"
  mkdir -p "$out"
  echo "==> $name -> $out"
  set +e
  "$CLI" bench "$MANIFEST" \
    --lang en \
    --terms "$TERMS_EN" \
    --boost "$BOOST" \
    --threads "$THREADS" \
    --out "$out" \
    "$@"
  local rc=$?
  set -e
  return $rc
}

mkdir -p "$OUT_ROOT"
fail=0

# Full English owner category (en-01…en-05), if present.
if ! run_one owner-en --categories en; then
  echo "warning: owner en clips missing or failed; see eval/owner-clips/README.md" >&2
  fail=1
fi

# Term-free owner split (en-01…en-03). Issue #38: report WER on these alone.
if ! run_one owner-en-termfree --clips en-01,en-02,en-03; then
  echo "warning: term-free owner clips missing" >&2
  fail=1
fi

# Term-heavy owner clip (en-04) for term recall / insertions with terms-en decoys.
if ! run_one owner-en-terms --clips en-04,en-05; then
  echo "warning: term-bearing owner clips missing" >&2
  fail=1
fi

# Public term-free English (FLEURS). Good insertion / WER regression check with terms-en decoys.
if ! run_one fleurs-en --categories fleurs-en; then
  echo "warning: fleurs-en missing; run: (cd eval && python3 -m tyst_eval fetch-fleurs)" >&2
  fail=1
fi

echo "Done. Summaries under $OUT_ROOT/*/summary.{md,json}"
echo "Gate: if term-free (owner-en-termfree and/or fleurs-en) WER rises with boost vs raw, set"
echo "  tyst_core::vocabulary::BOOST_ENGLISH = false"
echo "and amend docs/decisions/0001-inference-backend.md."
exit 0
