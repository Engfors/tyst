"""Regenerates the golden fixtures for `asr::spm` and `asr::boost` from the Python reference code.

    python3 -m pip install sentencepiece numpy
    python3 make_golden.py <tokenizer.model> <phrase_boost.py>

`tokenizer.model` and `phrase_boost.py` come from KlangAI/pianissimo-sv-onnx at tyst's pinned
revision. Writes `spm_golden.jsonl` (text and `SentencePieceProcessor.encode` IDs) and
`boost_golden.json` (`PhraseBoost.scores` for every state of seeded random trees).
"""
import importlib.util
import json
import random
import sys
from pathlib import Path

import sentencepiece as spm

HERE = Path(__file__).resolve().parent

FIXED = [
    "", " ", "  ", "Tyst", " Tyst ", "Tyst  dikterar", "Klang AI", "klang ai", "Terraform", "terra form",
    "HashiCorp", "GitHub", "git hub", "Kubernetes", "kubectl", "pull request", "state drift",
    "Åsa Öberg", "Ibrahimović", "Björn Ulvaeus", "Märta Ångström", "Jönköping", "Örebro", "Göteborg",
    "Zlatan Ibrahimović", "Dvořák", "Łódź", "São Paulo", "Ærøskøbing", "Øresund", "ﬁnans", "Ｔyst",
    "Ⅻ", "½", "x²", "№ 5", "é", "café", "naïve", "GPT-4o", "1,5 miljoner", "2026-10-09", "ID 123456",
    "C++", "C#", ".NET", "Node.js", "e-post", "AI/ML", "R&D", "\"citat\"", "«guillemets»", "‘quotes’",
    "a\tb", "a\nb", "a b", "a b", "a​b", "a　b", "­", "😀", "日本語", "Привет",
    "Ελληνικά", "<|endoftext|>", "<pad>", "a<pad>b", "ﬀ ﬃ", "Ǆ", "ℌ", "™", "…", "‐ – —",
]


def strings(vocab, rng, n):
    words = [p.replace("▁", "") for p in vocab if p.replace("▁", "").strip()]
    out = list(FIXED)
    while len(out) < n:
        k = rng.randint(1, 4)
        parts = rng.sample(words, k)
        sep = rng.choice([" ", "", " ", "-", "  "])
        s = sep.join(parts)
        if rng.random() < 0.3:
            s = s.capitalize()
        if rng.random() < 0.1:
            s = s.upper()
        out.append(s)
    return out


def load_phrase_boost(path):
    spec = importlib.util.spec_from_file_location("phrase_boost", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def boost_cases(pb, rng):
    cases = []
    for _ in range(40):
        vocab = rng.randint(4, 12)
        phrases = [[rng.randrange(vocab) for _ in range(rng.randint(1, 5))] for _ in range(rng.randint(1, 8))]
        tree = object.__new__(pb.PhraseBoost)
        tree.vocab_size = vocab
        tree.root = pb._Node(0, 0.0, 0.0, False)
        tree.root.fail = tree.root
        tree.nodes = [tree.root]
        for p in dict.fromkeys(tuple(p) for p in phrases):
            tree._add(list(p), 1.0, 2.0)
        tree._fill_fail()
        tree._cache = {}
        states = []
        for s in range(len(tree.nodes)):
            bonus, nxt = tree.scores(s)
            states.append({"bonus": [float(b) for b in bonus], "next": [int(x) for x in nxt]})
        cases.append({"vocab": vocab, "phrases": phrases, "states": states})
    return cases


def main():
    tokenizer, phrase_boost = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 1500
    out = Path(sys.argv[4]) if len(sys.argv) > 4 else HERE / "spm_golden.jsonl"
    sp = spm.SentencePieceProcessor(model_file=tokenizer)
    rng = random.Random(20261009)
    vocab = [sp.id_to_piece(i) for i in range(sp.get_piece_size())]
    with open(out, "w", encoding="utf-8") as f:
        for s in strings(vocab, rng, n):
            f.write(json.dumps({"text": s, "ids": sp.encode(s)}, ensure_ascii=False) + "\n")
    cases = boost_cases(load_phrase_boost(phrase_boost), random.Random(7))
    (HERE / "boost_golden.json").write_text(json.dumps(cases, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
