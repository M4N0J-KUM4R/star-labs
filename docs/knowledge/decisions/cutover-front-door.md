---
tags: [decision, strategy, cutover]
---
# Decision: cut over per route with a front door, not in one big bang (2026-11-27)

**Decision.** Production traffic moves route by route. The Rust application
answers the routes it owns; Canvas answers the rest. `rust-anything app
front-door` builds the axum proxy that does the routing, and
`app wire --route 'METHOD /path/{id}'` moves one route across at a time, with
the recorded corpus as the proof.

**Why.** 901 of the 1403 Canvas routes have no handler in the port, and the
Canvas frontend (5,459 TSX files) is a separate product surface. A big-bang
rewrite would have to clear all of that before any traffic could move.
Per-route cutover means the tail becomes a backlog worked against live traffic
instead of a gate that must be passed before shipping.

**What it needs.**
- The replay to be green for a route before it is wired across.
- The front door to proxy anything the Rust side does not own.
- A rollback that is one flag, not a redeploy.

**Related.** [[oracle-first]], [[continue-the-port-not-restart]].
