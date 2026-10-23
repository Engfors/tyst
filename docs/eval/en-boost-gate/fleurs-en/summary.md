# tyst-cli bench · 2026-10-17T06:27:33.337836248+00:00

- Platform: `linux x86_64`, 4 threads, engine parakeet-v3-int8@8f23f0c
- Clips: 25
- Model load 2.1 s, RSS after load 1012 MB, peak 1220 MB
- Decode RTF 0.045 (decode time / speech time)

WER, term recall and term exact spelling in %. Baseline: Phase 0 harness, greedy, only-sv.

Inserted: term occurrences in the transcripts beyond those in the references (decoys count every time).

| category | post | clips | WER | baseline WER | Δ pp | term recall | term exact | inserted | RTF |
|---|---|---|---|---|---|---|---|---|---|
| ALL | boost 0.25 | 25 | 25.5 | – | – | – | – | 0 | 0.046 |
| ALL | boost 0.25+vocab | 25 | 25.5 | – | – | – | – | 0 | 0.046 |
| ALL | boost 0.5 | 25 | 25.5 | – | – | – | – | 0 | 0.045 |
| ALL | boost 0.5+vocab | 25 | 25.5 | – | – | – | – | 0 | 0.045 |
| ALL | boost 1 | 25 | 26.1 | – | – | – | – | 0 | 0.045 |
| ALL | boost 1+vocab | 25 | 26.1 | – | – | – | – | 0 | 0.045 |
| ALL | raw | 25 | 25.7 | – | – | – | – | 0 | 0.045 |
| ALL | vocab | 25 | 25.7 | – | – | – | – | 0 | 0.045 |
| fleurs-en | boost 0.25 | 25 | 25.5 | – | – | – | – | 0 | 0.046 |
| fleurs-en | boost 0.25+vocab | 25 | 25.5 | – | – | – | – | 0 | 0.046 |
| fleurs-en | boost 0.5 | 25 | 25.5 | – | – | – | – | 0 | 0.045 |
| fleurs-en | boost 0.5+vocab | 25 | 25.5 | – | – | – | – | 0 | 0.045 |
| fleurs-en | boost 1 | 25 | 26.1 | – | – | – | – | 0 | 0.045 |
| fleurs-en | boost 1+vocab | 25 | 26.1 | – | – | – | – | 0 | 0.045 |
| fleurs-en | raw | 25 | 25.7 | – | – | – | – | 0 | 0.045 |
| fleurs-en | vocab | 25 | 25.7 | – | – | – | – | 0 | 0.045 |

21 manifest entries skipped (missing files).
