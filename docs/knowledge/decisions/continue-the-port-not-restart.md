---
tags: [decision, strategy]
---
# Decision: continue the port, do not restart it (2026-11-27)

**Decision.** Keep `star-labs` as the target. Do not re-derive the migration from
scratch, with or without rust-anything.

**Why.**

1. **rust-anything's unit loop cannot port a Rails monolith.** Its Ruby mode is
   magnus-based FFI: extract a Ruby method, compile it into Rust, call back into
   Ruby from Rust. That keeps the Ruby runtime resident forever — exactly the
   footprint the rewrite exists to remove. The applicable part of the tool is the
   whole-app HTTP loop (`record`/`replay`/`front-door`/`wire`), which *verifies* an
   existing implementation; it does not write one.
2. **The port's defect class is verification, not generation.** 1403 routes were
   registered but only 7 had a checked response shape; the binary panicked at
   startup and CI never noticed; three `star-accounts` assertions encoded
   hardcoded guesses that the recorded Canvas body contradicted; 266 of the first
   430 recorded bodies were error envelopes because nobody set up an oracle.
   Re-running a different generator produces none of those checks.
3. **What is in the port is real work.** 153k LOC, 2,660 `sqlx` query sites against
   232 inline `json!` literals, 1,414 tests in 170 files, 502 of 1403 Canvas
   routes implemented, 149+ migrations.

**The measurement that settled it.** One `app replay` against the current binary
diffs all recorded bodies. If most match, continue. If most mismatch
structurally, rewrite the handlers per family — still on the existing skeleton,
which is cheaper than 1403 routes from scratch.

**Consequence.** The recorded corpus, not a code generator, is the engine of this
migration. See [[oracle-first]].
