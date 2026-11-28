---
tags: [index, map-of-content]
---
# Knowledge vault — read this first

This is the map of content for the Canvas → Rust migration. Every note is
either a **generated fact** or a **hand-written trap/decision**. An agent
working on this migration should read this file, then the two or three notes it
points at — **not** grep the tree.

The vault lives in the repo on purpose: it is versioned, reviewed, CI-checked,
and readable by both the local agents (Claude Code, Codex) and the GitHub-only
Qwen workers.

## The five-minute path

| You are working on | Read, in this order |
|---|---|
| **a Canvas route** | `briefs/<family>.md` → [[facts/route-map]] → the Canvas file it names → [[facts/schema-truth]] for that table |
| **a table's fields** | [[facts/schema-truth]] for that table → the recorded body in `migration/rust/app-cases/http/` → the Canvas model |
| **a failing parity case** | `migration/rust/app-cases/replay-report.md` → the case's `.toml` (what Canvas answered) → the handler |
| **the current state of the port** | [[facts/parity-state]] → [[facts/capability-ledger]] for the per-capability verdict |
| **anything that has bitten you** | `traps/` — read the list below before you start |
| **"why is it done this way?"** | `decisions/` |

## Generated facts

Regenerate them; never edit them by hand.

```sh
cd star-labs && python3 tools/knowledge/gen_knowledge.py          # write
cd star-labs && python3 tools/knowledge/gen_knowledge.py --check  # CI: stale?
```

| Fact | Source of truth | What it answers |
|---|---|---|
| [[facts/route-map]] | `migration/parity/canvas_manifest.json` + `coverage.json` + the port's own registrations | Which crate owns this route, which Canvas file to read, is it implemented, does the replay pass |
| [[facts/schema-truth]] | the recorded Canvas bodies + `migration/crates/star-db/migrations/` | Which columns this table still owes Canvas |
| [[facts/parity-state]] | `coverage.json` + `replay-report.md` | How far along the port actually is |
| [[facts/capability-ledger]] | the parity manifest + the corpus + the replay report | Every Canvas capability: where the port's code is, what evidence exists, what still differs |
| [[facts/gap-report]] | `parity/canvas-*.json` (Canvas introspection) vs the tree | What the port still owes Canvas: routes, permissions, columns, GraphQL |

## Per-family briefs — start here, not in the repo

`docs/knowledge/briefs/<family>.md` is a complete slice in one file: the routes
the family owns, whether the port has them, **the Canvas controller file to
read**, the Rust crate to change, the recorded case that shows what the answer
must look like, the traps that apply, and the checklist.

    courses.md  users.md  accounts.md  enrollments_api.md  groups.md  ...

A worker that gets a brief should not need to explore the repository at all.

## Traps — the things that will bite you

Read the relevant one before you touch the area. Each is short: symptom,
cause, fix.

| Trap | One line |
|---|---|
| [[traps/axum-merge-semantics]] | axum 0.8 **merges** same-path/different-method silently; it panics only on same-method or same-matchit-shape |
| [[traps/double-merge-star-content]] | `star_content` already merges `star_pages`/`star_modules`/`star_announcements` — do not merge them again |
| [[traps/canvas-root-account]] | Canvas' `root_account?` means `root_account_id.zero()`, not `== id` |
| [[traps/canvas-schema-differences]] | no `accounts.account_type`, no `users.login_id`, `GroupMember` is `GroupMembership` |
| [[traps/canvas-special-accounts-production]] | `Account.default` must be force-created in production, or every request 500s |
| [[traps/canvas-needs-rev-manifest]] | `db:migrate` aborts without `public/dist/rev-manifest.json` |
| [[traps/bundler-multilock-is-a-plugin]] | it is a bundler **plugin**, not a gem — and it drags in bundler 4 |
| [[traps/corpus-token-placeholder]] | the committed corpus stores `Bearer __REPLAY_TOKEN__`, never a live token |
| [[traps/canvas-response-headers]] | Canvas' ~19 response headers — FIXED by `star-canvas-response` |
| [[traps/canvas-access-token-auth-model]] | Canvas resolves the caller from an `access_tokens` row, not from a token's contents |
| [[traps/canvas-meta-volatile-fields]] | `x-canvas-meta` mixes the contract with process diagnostics; inline `[..]` wildcards are not honoured |
| [[traps/rust-errors-are-plain-text]] | Canvas' error body shape depends on the failure kind — 401/403/404/400/500 each differ |
| [[traps/axum-json-extractor-415]] | `Json<T>` answers 415 before your permission check runs — use `ApiJson<T>` |
| [[traps/canvas-permission-order]] | 401 vs 403 vs 404 is decided by the **order** of Canvas' checks, and permissions are contextual |
| [[traps/serde-json-sorts-keys]] | `serde_json::Map` sorts keys alphabetically, so a hand-built body comes out in the wrong order |
| [[traps/replay-fixture-rules]] | the replay database must hold the state the corpus was recorded in, or every case 401s |
| [[traps/canvas-token-self-expiry]] | a route that ends sessions must not end the replay's own token |
| [[traps/ci-never-assembled-the-router]] | `cargo test -p` never assembles the app router — that is why the boot bug was invisible |
| [[traps/star-http-is-the-only-binary]] | the binary is `star-http`; `PORT` is the fallback for `STAR_PORT` |
| [[traps/front-door-delegation]] | the front door delegates on axum's fallback **and** its method-not-allowed fallback; Canvas answers 404, never 405 |
| [[traps/front-door-original-path]] | `nest("/api/v1")` strips the prefix before the fallback runs — forward `OriginalUri`, not `parts.uri` |
| [[traps/replay-gate-reads-git]] | the replay overwrites the committed report; the gate must read the baseline out of git |
| [[traps/role-model-vs-canvas-role-overrides]] | the port's role model is stricter than Canvas' role-override defaults (students, `read_roster`) |
| [[traps/api-resources-draw-put-not-patch]] | Canvas' API `resources` draws PUT only — the port registers 9 API PATCH routes Canvas does not serve |
| [[traps/account-gated-permissions]] | 45 permissions hang on an account feature flag the port never grants — always denied |

## Decisions — why things are the way they are

| Decision | One line |
|---|---|
| [[decisions/continue-the-port-not-restart]] | continue the port; rust-anything's unit loop cannot port a Rails monolith |
| [[decisions/oracle-first]] | the recorded Canvas corpus is the behavioural spec |
| [[decisions/chaining-not-deleting]] | chain same-path routes; never drop a Canvas method |
| [[decisions/vault-is-a-cache-not-a-store]] | facts are generated and CI-diffed; only traps and decisions are hand-written |
| [[decisions/cutover-front-door]] | cut over per route behind a front door, not in one big bang |

## How this compares to the research

The vault follows the *information architecture* the agent-memory literature
argues for — atomic notes, explicit links, and separated note types — but stops
short of the parts that only make sense for a single agent's private memory:

- **A-MEM** (NeurIPS 2025, arXiv 2502.12110) — Zettelkasten linking and
  structured note attributes: adopted. Its *memory evolution*, where adding a
  memory retroactively rewrites older notes: **not adopted** — an agent
  silently mutating shared project knowledge is untracked divergence. Here,
  evolution is a reviewed PR.
- **MAGMA** (ACL 2026, arXiv 2601.03236) — orthogonal stores instead of one
  entangled graph: adopted, as `facts/` / `traps/` / `decisions/`. Its
  policy-guided traversal is the route map: given a family, the chain of files
  to read.
- **Synapse** (Findings of ACL 2026) — activation over a link graph, and the
  "contextual tunneling" failure it names: adopted conceptually. The index here
  is the cheap version — it forces breadth-first reading instead of letting an
  agent tunnel into one crate.
- **AgeMem** (ACL 2026) — the five memory operations (store, retrieve, update,
  summarize, discard) are a good CRUD surface for the vault. Its **RL-trained
  write policy is not adopted**: our write policy must be deterministic, so the
  facts are generated from committed artefacts rather than chosen by a model.
- The surveys agree on the operational half: hand-maintained file memory
  degrades at scale. Hence: generate what can be generated, CI-check the rest.

## Conventions

- One note = one idea, densely linked. Link with `[[note-name]]`.
- Facts are generated; traps and decisions are written by hand.
- A trap that no longer applies is deleted, not left to rot.
- Every note that names a file names it as `path` and, where it helps, a line.
