---
tags: [decision, knowledge, ci]
---
# Decision: the knowledge vault is a CACHE over committed artifacts, not a store (2026-11-27)

**Decision.** `docs/knowledge/` has two kinds of note:

- **Facts — generated, never hand-written.** `facts/route-map.md`,
  `facts/schema-truth.md`, `facts/parity-state.md` are produced by
  `tools/knowledge/gen_knowledge.py` from artefacts that are already committed
  (`migration/parity/*.json`, `migration/rust/app-cases/`, the migrations).
  CI runs the generator with `--check` and fails if the committed copy differs.
- **Traps and decisions — hand-written.** These record things no artefact can
  tell you: what bit us, and why we chose what we chose.

**Why.** A note that holds a fact not derivable from a committed artefact will
drift, and no retrieval cleverness fixes drift. The research on agent memory
makes the same point from the other side: A-MEM's memory is the agent's system of
record and it evolves in place, which is right for one agent's private memory
and wrong for shared, audited project knowledge — an agent silently rewriting a
shared note is untracked divergence with no review. Here, "evolution" must be a
reviewed PR.

The surveys agree on the operational half: naive hand-maintained file memory
degrades in retrieval accuracy at scale. So: generate what can be generated, and
CI-check the rest.

**Consequence.** The vault cannot rot silently. `gen_knowledge.py --check` is a
CI job. A trap note that no longer applies is a PR that deletes it.

**Related.** [[oracle-first]], [[continue-the-port-not-restart]].
