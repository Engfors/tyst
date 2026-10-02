# Phase 0 owner clips: interview manuscripts

Scripted, fictional content only. Recordings and edited references stay outside git.

20 short scripted interviews, one per clip in `eval/manifest.toml` (branch `claude/phase0-eval-jc0nx5`).
Each manuscript is also the reference transcript: `make_refs.py` turns it into `references/<id>.txt`.

| Category | Clips | What the clip tests |
|---|---|---|
| sv | sv-01 … sv-05 | Everyday Swedish, place names, numbers, one fast back-and-forth (sv-05) |
| en | en-01 … en-05 | Swedish-accented English; en-04 uses every term from `terms.toml` as an English baseline |
| mixed | mixed-01 … mixed-05 | Language switch between speakers: every turn (01, 04), one switch mid-clip (02), rapid short turns (03), English start and end around a Swedish body (05) |
| sv-terms | sv-terms-01 … sv-terms-05 | Swedish sentences with the English terms, base forms plus Swedish inflections (pipelinen, deploya, containrar) |

Estimated lengths are 50 to 100 s each (`python3 make_refs.py` prints them), well inside the 30 s to 3 min rule.

## Roles

`I` is the interviewer and `G` the guest. Two people, ideally Emil plus one other person, so the set has two voices.
Swap roles every few clips so both voices get Swedish, English and term-heavy material.
In mixed clips the tag after the letter, `[sv]` or `[en]`, is the language of that turn.
Lines in parentheses are stage directions and are not spoken.

## Recording

1. Quiet room, one laptop or phone microphone between you, about as far away as in a real meeting. Don't use a headset mic for both; the point is meeting-like audio.
2. One file per clip, named exactly `<id>.wav` (or `.m4a`/`.flac`; then change `audio` in the manifest).
3. Leave about a second of silence at the start and end. Don't talk over each other; overlapping speech makes the reference ambiguous.
4. Read naturally, at conversational speed. Small slips, rewordings and "eh" are fine and even useful, but write them into the reference afterwards (step 6).
5. If a take goes badly, just redo the clip.

## After recording

6. Listen to each clip and edit `references/<id>.txt` so it matches exactly what was said. Punctuation and case don't matter for WER; words do. Write numbers the way they are written in the manuscript (as words); the harness doesn't treat "18" and "arton" as equal.
7. For mixed clips, note the second where each language block starts and run
   `python3 make_refs.py spans <id> <clip length in s> <start of block 2> <start of block 3> …`.
   The plain `python3 make_refs.py` run lists the blocks in order. Paste the printed `lang_spans` line into the clip's entry in `manifest.toml`. Rough times (±1 s) are fine.
8. Copy `<id>.wav` and `references/<id>.txt` to `~/tyst-eval/clips/owner/`.

Recordings and references stay on your machine; nothing here is committed with audio.

## Term notes

- Term accuracy counts only whole-word matches of the terms in `terms.toml`. Inflected forms such as "pipelinen" or "containrar" count for WER only, so the sv-terms manuscripts use each term's base form several times as well.
- sv-terms-03 and sv-terms-05 use "rollback", which is not in `terms.toml` yet; it should be added.
