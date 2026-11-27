---
tags: [decision, strategy, harness]
---
# Decision: the recorded Canvas corpus is the spec (2026-11-27)

**Decision.** The behavioural contract for every route is what a live Canvas
answered, captured by `rust-anything app record`. The corpus lives in
`migration/rust/app-cases/http/` and CI replays it against the Rust build.

**Why.** The plan's own measurement said the route-count phase was over
(existence, not behaviour) and that 542 columns were missing across 67 tables.
Nothing in the repo could say *which* field of *which* route was wrong. The
corpus does: it is a per-route, machine-checked expectation with a file-and-line
diff attached.

**How it is kept honest.**
- The corpus is committed, so a replay needs no Canvas.
- The replay report is committed, and CI fails when a fresh run disagrees with it
  (`git diff --exit-code`) — so a port that silently loses behaviour fails the
  build.
- The corpus never contains a live token: it stores `Bearer __REPLAY_TOKEN__`.
  See [[corpus-token-placeholder]].
- Facts derived from it (which columns a table owes Canvas) are **generated**,
  not hand-written, and CI-diffed. See [[vault-is-a-cache-not-a-store]].

**Consequence.** "35.8% implemented" stops being a guess. The first replay said
0/430 passed — and the reason was one missing middleware affecting every case,
not 430 separate bugs. See [[canvas-response-headers]].

**Related.** [[continue-the-port-not-restart]], [[cutover-front-door]].
