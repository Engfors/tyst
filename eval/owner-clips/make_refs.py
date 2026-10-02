#!/usr/bin/env python3
"""Turn manuscripts/<id>.md into references/<id>.txt and print length estimates.

  python3 make_refs.py                     # write all references, print words/estimated seconds
  python3 make_refs.py spans <id> <duration_s> <t2> [<t3> ...]
      print a lang_spans line for a mixed clip: give the clip length and the second at which
      each language block after the first starts (blocks are listed by the plain run).

A reference file holds only the spoken words, one turn per line, no speaker labels.
After recording, edit references/<id>.txt so it matches what was actually said.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).parent
TURN = re.compile(r"^([A-Z])(?:\s*\[(sv|en)\])?:\s*(.+)$")
WORDS_PER_S = 2.5   # relaxed conversational pace
GAP_S = 0.5         # pause between turns


def turns(path):
    body = path.read_text(encoding="utf-8").split("\n---\n", 1)[1]
    for line in body.splitlines():
        m = TURN.match(line.strip())
        if m:
            yield m.group(1), m.group(2), m.group(3)


def blocks(ts):
    out = []
    for _, lang, text in ts:
        if out and out[-1][0] == lang:
            out[-1][1] += len(text.split())
        else:
            out.append([lang, len(text.split())])
    return out


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "spans":
        cid, dur, starts = sys.argv[2], float(sys.argv[3]), [float(x) for x in sys.argv[4:]]
        bl = blocks(list(turns(ROOT / "manuscripts" / f"{cid}.md")))
        if len(starts) != len(bl) - 1:
            sys.exit(f"{cid} has {len(bl)} language blocks, so give {len(bl) - 1} start times")
        edges = [0.0, *starts, dur]
        spans = ", ".join(f'[{a:.1f}, {b:.1f}, "{l}"]' for (l, _), a, b in zip(bl, edges, edges[1:]))
        print(f"lang_spans = [{spans}]")
        return
    (ROOT / "references").mkdir(exist_ok=True)
    for md in sorted((ROOT / "manuscripts").glob("*.md")):
        ts = list(turns(md))
        (ROOT / "references" / f"{md.stem}.txt").write_text(
            "\n".join(t for _, _, t in ts) + "\n", encoding="utf-8")
        n = sum(len(t.split()) for _, _, t in ts)
        est = n / WORDS_PER_S + GAP_S * (len(ts) - 1)
        extra = ""
        if ts[0][1]:
            extra = "  blocks: " + " → ".join(f"{l} ({w} words)" for l, w in blocks(ts))
        print(f"{md.stem:12} {len(ts):3} turns {n:4} words  ~{est:4.0f} s{extra}")


if __name__ == "__main__":
    main()
