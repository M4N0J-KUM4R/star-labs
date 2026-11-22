# Verification receipt — PR #200 "star-db: the full Canvas schema"

- **Receipt id:** `pr-200-schema-parity-verify-001`
- **Date:** 2026-11-22
- **Base:** `main` @ `316a81f` (merge of PR #211). Schema-stale check: zero
  commits touch `migration/crates/star-db/**` between `316a81f` and current
  head `42aad72` (the deltas are UI PR #203 and CI guard PR #214), so the
  verdict applies unchanged to current main.
- **Verdict:** PR #200's claims **hold on current main**. 8 exceptions on the
  PR #200 cohort, every one attributable to later deliberate work (migration
  0228 / the LTI PRs) or to reference-model artifacts — none is silent drift.
  One new incident found during the audit (migration ledger broken on main) —
  fixed in PR #215.

## Method

The 139-file migration chain was replayed in filename order (one transaction
per file — exactly what sqlx's `MIGRATOR` runs) into a fresh PostgreSQL 16.4,
then the **live** schema was compared column-by-column against the 302-table
Canvas reference (`canvas_schema.json`, simulated from the canvas-lms
`db/migrate` replay). No regex guessing on the live side:
`pg_attribute` / `format_type` / `pg_constraint` / `pg_indexes` are the ground
truth. Harness: workspace-side script set (`apply_migrations.py`,
`verify_schema_parity.py`, `audit_star_only.py`); machine-readable outputs
(full per-table diff JSON, star-only classification matrix) retained with the
audit record.

## Claim-by-claim evidence

| Claim made by PR #200 | Verified |
|---|---|
| every missing Canvas table added (220 tables, additive only) | **YES** — 302/302 reference tables exist live, 0 missing, 0 primary-key issues |
| Rails column types / NULL constraints / defaults on those tables | **YES** — 0 missing columns, 0 default mismatches; 6 type + 3 NOT NULL deviations, all from *later* migration 0228 (below) |
| 693 secondary indexes with Rails names | **YES** — 998 reference-named indexes checked, **1** not present under its Canvas name (mangled, below) |
| 453 foreign keys | **YES** — 688 reference FK pairs checked, **2** deviations on the new-table cohort, both explained (below) |
| whole 137-file chain applies to a fresh PG16 | **YES** — now 139 files (0100 + 0228 added since PR #200), all applied cleanly: 370 tables, 3782 columns, 1426 indexes, 704 FKs |

## Cohort split (why totals differ from the headline)

PR #200 was additive-only: it added tables star-db did not already define and
left the 82 pre-existing **authored** tables (0001–0222) untouched:

| Cohort | Tables | Missing cols | Extra cols | Type mm | NOT NULL mm | Missing named idx | Missing FKs |
|---|---:|---:|---:|---:|---:|---:|---:|
| canvas-schema tables (0223–0225) | 220 | **0** | 2 | 6 | 3 | 1 | 2 |
| authored tables (0001–0222) | 82 | 740 | 132 | 414 | 165 | 366 | 132 |

The authored-cohort numbers are out of PR #200's scope **by design**, not
regressions: they measure how far the hand-written tables (assignments −60
cols, courses −47, discussion_topics −41, quizzes −36, submissions −36,
users −36, accounts −33, …) diverge from the Canvas reference today. If full
column parity on legacy tables ever becomes a goal, that is the backlog
(~740 `ADD COLUMN`s + ~132 type/null alignments, each needing a check that no
existing Rust code breaks) — recommended as a follow-up `epic:infra` issue,
not urgent: no current route reads those columns.

## The 8 exceptions on the PR #200 cohort, all explained

1–6. `lti_context_controls`, `lti_overlay_versions`,
`lti_registration_history_entries` — 6× `created_at`/`updated_at`
`timestamp`→`timestamptz`, 2× `DROP NOT NULL` (`course_id`/`account_id` on
context controls), 2 extra columns (`overlay`, `registration_id` on overlay
versions). All are migration 0228 (`lti_registrations_deep`, from the LTI
PRs #201/#205/#210): the Rust code wants `DateTime<Utc>`, and exactly one of
account/course scopes a control. Documented in the migration itself.

7. `index_LOQR_on_learning_outcome_result_id` — the index exists but under
the name `index______on_learning_outcome_result_id`: the 0226 generator's
identifier sanitizer stripped the uppercase `LOQR` token from Canvas's custom
index name. Harmless (identical index behavior), cosmetic name deviation,
one-line rename if ever desired.

8. `lti_context_controls.deployment_id` FK — 0228 deliberately rewired it
from `context_external_tools` to this workspace's `lti_deployments`
(migration 0078), with an in-migration comment. The other "missing" FK
(`lti_assets` → submissions, `column: null`) is an artifact of the reference
model (unresolved polymorphic FK), not of the database.

Also explained, not counted: `page_views.request_id` is NOT NULL live because
PostgreSQL enforces it implicitly as the primary key — the reference's
nullable flag for it is a simulation nuance.

## Star-only tables (68) — the additive-only rule held

All 68 live tables with no Canvas-reference counterpart were classified
(creating migration, best fuzzy Canvas match, canvas-src textual evidence,
Rust usage). 52 have either a fuzzy Canvas counterpart or direct canvas-src
textual evidence (e.g. `external_tools` ↔ `context_external_tools`,
`polls`/`poll_choices`/`poll_sessions`/`poll_submissions` ↔ Canvas's
`polling/*` models, `outcome_groups`/`outcome_links`/`outcome_alignments` ↔
Canvas models of the same names — Canvas keeps those in a different table
layout, star flattens them).

16 have no Canvas textual evidence at all — this workspace's own inventions,
all actively used by Rust code: `announcement_reads`,
`assignment_annotations`, `calendar_user_preferences`,
`discussion_subscriptions`, `document_annotation_reads`,
`grading_scheme_entries`, `group_invitations`, `job_clusters`,
`learning_object_date_overrides`, `lti_data_service_subscriptions`,
`module_item_completions`, `notification_preferences`,
`password_reset_tokens`, `rubric_ratings`, `submission_files`,
`user_colors`. These model Canvas concepts Canvas itself stores differently
(read-state tables, subscription state, reset tokens, rubric ratings,
grading-scheme rows, jobs-v2 clusters). None is dead weight; PR #200
correctly left them alone under the additive-only rule.

## New finding — migration ledger broken on main → fixed in PR #215

`python3 orchestration/check-migration-ledger.py` failed on the audited main
(and still fails on `42aad72`):

```
- migration file 0100_*.sql exists but has no ledger reservation
- free number 0100 has a merged migration file
```

Cause: PR #212 (polling API, issue #89, merged 2026-11-21T20:51Z) used the
ledger's free hole `0100` for `0100_polling.sql` but nobody moved `0100` from
the `free` list to a merged reservation — and no CI workflow ran the checker,
so nothing caught it. **Fixed in PR #215** (branch `infra/ledger-0100-fix`):
`0100` free → merged reservation (issue 89), and the ledger check now runs
inside the required `test` gate job of `rust-migration.yml`, so any future PR
that adds a migration without a reservation fails the required check before
merge.

## Reference-model artifacts worth knowing (canvas_schema.json)

For anyone re-running this verification, three quirks of the reference
simulator so its output is not misread as live drift:

- The `null` field encodes NOT NULL (true = NOT NULL), not nullability.
  Confirmed against Canvas baseline `create_table` blocks (e.g.
  `accounts.root_account_id` `null: false` → ref `null: true`).
- 45 dangling FK targets, all pluralizer bugs in the simulator: `sis_batchs`
  (16), `quizs` (7), `discussion_entrys` (5), `group_categorys` (4),
  `progresss` (2), `custom_grade_statuss` (2), `user_merge_datas` (2), and 8
  singles. Rails pluralizes these `…batches/…quizzes/…entries/…categories/
  …progresses/…statuses/…data`; the live DB correctly has no such tables.
- `lti_assets` carries one unresolved polymorphic FK (`column: null`).

None of these affects the verdict: the live side is the source of truth and
the affected comparisons were excluded or explained.

## Conclusion

All five PR #200 claims hold on current main; the 8 exceptions are attributed
and documented, the 68 star-only tables are justified under the additive-only
rule, and the one incident uncovered by the audit (ledger 0100) is fixed by
PR #215. No code changes are required by this receipt; it is the audit
record only.
