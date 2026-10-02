# Architecture decision records

Short ADRs, one per decision. Phase 0 decisions are binding for later phases (SPEC section 0).

| # | Decision | Status |
|---|---|---|
| [0001](0001-inference-backend.md) | Inference backend: ONNX Runtime (`ort`) with a TDT greedy decoder | Accepted |
| [0002](0002-routing-strategy.md) | Routing: fixed Pianissimo, English on request | Accepted |
| [0003](0003-language-id.md) | Language ID: none in v1 | Accepted |
| [0004](0004-segmentation.md) | Segmentation: SPEC defaults, partial interval grows with the buffer | Accepted |

Template: Context · Decision · Consequences · Evidence. Status is Proposed until the owner confirms,
then Accepted.
