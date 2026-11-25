# Background Job Worker — Design Spec (MIG-04a)

Status: proposal for coordinator review. Documentation only — no Rust, SQL, or CI
changes are made by this spec.

Ground truth:
- Canvas: `canvas-src/` (upstream pinned at 1c9f0bb). Every Canvas claim below
  cites `file:line` in that tree.
- Star: this worktree (`p5/infra-jobs-spec`, branched from origin/main 70792ef).

Scope: the missing background-job worker for the `jobs` surface that `star-jobs`
already exposes. Acceptance shape from the migration plan: a queued job runs
exactly once; a failing job stops at its limit; a killed worker's job is
recovered; cancel holds (`full-migration-plan.md:96`).

---

## 1. Canvas facts

### 1.1 Which table, which columns

Canvas' job backend is `Delayed::Job`, persisted in the `delayed_jobs` table,
created by `canvas-src/db/migrate/20101216224513_create_delayed_jobs.rb:26`:

| Column | Line | Notes |
|---|---|---|
| `priority` | :28 | default 0; lower runs first (index order below) |
| `attempts` | :30 | default 0; incremented per failed run |
| `handler` | :32 | "YAML-encoded string of the object that will do work" (:31) |
| `last_error` | :34 | text of the last failure |
| `queue` | :36 | NOT NULL |
| `run_at` | :39 | NOT NULL; future = scheduled, past/now = due |
| `locked_at` | :41 | set while a worker runs the row |
| `failed_at` | :43 | set when retries are exhausted |
| `locked_by` | :45 | worker identity while locked |
| `tag` | :49 | indexed; the job's display name (e.g. `Class#method`) |
| `max_attempts` | :50 | per-job override of the worker default |
| `strand` | :51 | serialization group |
| `next_in_strand` | :52 | default TRUE; FALSE = holding behind a running strand slot |
| `shard_id` | :53 | cluster/shard routing |
| `source` | :54 | enqueue origin |
| `max_concurrent` | :55 | default 1; strand parallelism |
| `expires_at` | :56 | hard deadline |
| `strand_order_override` | :57 | default 0; ordering hint inside a strand |
| `singleton` | :58 | at-most-one-unique-work marker |

The failed-job archive is a second table `failed_jobs`, same migration
(`20101216224513_create_delayed_jobs.rb:249`), carrying `failed_at` (indexed,
:257), `original_job_id` (:266) and `requeued_job_id` (:271).

Correction to the task brief: the `delayed_jobs`/`failed_jobs` DDL is **not** in
`20101210192618_init_canvas_db.rb`; that migration only references
`delayed_jobs_shard_id` on `switchman_shards`
(`canvas-src/db/migrate/20101210192618_init_canvas_db.rb:5094-5099`). The full
table definitions live in `20101216224513_create_delayed_jobs.rb` as cited above.

The Ruby backend library is the `switchman-inst-jobs` gem, pinned `~> 4.0`
(`canvas-src/Gemfile.d/app.rb:29`), resolved 4.3.2 (`canvas-src/Gemfile.lock:1204`).
Its source is **not vendored** in `canvas-src/` (checked `gems/`, `vendor/`).
Any claim that depends on gem internals is marked **[gem, unverified locally]**.

### 1.2 Payload format

- `handler` holds a YAML serialization of the Ruby object that performs the work
  (`20101216224513_create_delayed_jobs.rb:31-32`).
- Canvas enqueues plain Ruby worker objects, e.g.
  `Delayed::Job.enqueue(work, **enqueue_args)` (`canvas-src/app/models/progress.rb:130`)
  and `Delayed::Job.enqueue(new(content_migration.id), ...)`
  (`canvas-src/lib/canvas/migration/worker/zip_file_worker.rb:76`).
- `tag` is the human-readable job name surfaced by the admin API and used for
  filtering/lifecycle, e.g. `Delayed::Job.running.where(tag: "SubmissionLifecycleManager#recompute_for_sis_import")`
  (`canvas-src/lib/submission_lifecycle_manager.rb:159`) and
  `Delayed::Job.where(strand: ...).destroy_all` (`canvas-src/app/models/quizzes/quiz_statistics.rb:122`).
  Derivation of `tag` from the payload object is **[gem, unverified locally]**.
- Enqueue sugar (`.delay`, `handle_asynchronously`) also writes these rows, e.g.
  `handle_asynchronously :create_observer_alerts, priority: Delayed::LOW_PRIORITY, max_attempts: 1`
  (`canvas-src/app/models/announcement.rb:213`).

### 1.3 States

Canvas has no `workflow_state` column; the admin Jobs UI derives four buckets
from the columns above:
- **queued**: due and unlocked — the claim index is partial on
  `queue = 'canvas_queue' AND locked_at IS NULL AND next_in_strand`
  (`20101216224513_create_delayed_jobs.rb:62-64`).
- **running**: `locked_at`/`locked_by` set.
- **future**: `run_at` in the future.
- **failed**: row moved to `failed_jobs` with `failed_at` set
  (`20101216224513_create_delayed_jobs.rb:257`; controller comment
  `canvas-src/app/controllers/jobs_v2_controller.rb:293-299`).

### 1.4 Retry and backoff math

- Each failed run is bookkept in `attempts` (:30), `last_error` (:34) and the
  rescheduled `run_at` (:39) — storage defined at
  `20101216224513_create_delayed_jobs.rb:30, :34, :39`; the reschedule itself
  executes in gem code **[gem, unverified locally]**.
- `max_attempts` comes from the job row when set (`:50`), else the worker
  default `Delayed::Settings.max_attempts = 1`
  (`canvas-src/config/initializers/delayed_job.rb:77`). Callers do pass explicit
  values: `max_attempts: 1` (`canvas-src/app/models/progress.rb:125`,
  `canvas-src/app/models/content_migration.rb:419`) and `max_attempts: 10`
  (`canvas-src/app/models/course.rb:1747`).
- Backoff: the delayed-job convention `new run_at = now + attempts^4 + 5`
  seconds appears verbatim in Canvas retry code —
  `canvas-src/lib/turnitin/outcome_response_processor.rb:85` and
  `canvas-src/lib/basic_lti/basic_outcomes.rb:269`. The worker-level reschedule
  formula itself lives in the gem (**[gem, unverified locally]**; see open
  question 5.5).
- At the limit: the row is archived into `failed_jobs` with `original_job_id`
  set (`20101216224513_create_delayed_jobs.rb:249-271`), keeping `last_error`
  for inspection. Two in-repo corroborations: the requeue comment "given a job
  that exhausted all of its max_attempts and has been put into failed_jobs"
  and `Delayed::Job::Failed.find(params[:id])` at
  (`canvas-src/app/controllers/jobs_v2_controller.rb:293-299`). Exception: jobs
  whose final error is `Delayed::Backend::RecordNotFound` are **discarded, not
  kept**, via `Delayed::Worker.on_max_failures`
  (`canvas-src/config/initializers/delayed_job.rb:113-117`).
- Requeue copies the failed payload into a fresh queued row and stamps
  `requeued_job_id` on the failed row (`20101216224513_create_delayed_jobs.rb:271`;
  flow `canvas-src/app/controllers/jobs_v2_controller.rb:293-299`); the requeued
  row gets one more attempt even past `max_attempts` (same comment block).

### 1.5 Strand, queue, priority, singleton

- Single default queue: `Delayed::Settings.queue = "canvas_queue"`
  (`canvas-src/config/initializers/delayed_job.rb:80`); production pool config
  runs 2 workers with `max_priority: 10` plus 4 unbounded workers on that queue
  (`canvas-src/config/delayed_jobs.yml:2-7`).
- Claim order is `(priority, run_at, id)` ASC — partial index
  `get_delayed_jobs_index` on `%i[priority run_at id]` where the row is due,
  unlocked and `next_in_strand` (`20101216224513_create_delayed_jobs.rb:62-64`).
- **Strand**: at most `max_concurrent` rows of a strand (default 1, `:55`) may
  be `next_in_strand = TRUE` at once. Enforced by DB triggers, not worker code:
  - insert trigger: a new stranded row gets `next_in_strand := false` when the
    strand already has `max_concurrent` runnable rows
    (`20101216224513_create_delayed_jobs.rb:106-114`, trigger created at :128);
  - delete trigger: when a runnable stranded row finishes (is deleted), waiting
    rows are promoted in `(strand_order_override, id)` ASC order, up to the free
    concurrency slots (`:132-231`, trigger at :231);
  - both paths take a per-strand `pg_advisory_xact_lock(half_md5_as_bigint(...))`
    (`:108`, `:154`).
- **n_strand**: `Delayed::Settings.num_strands` resolves the
  `<strand>_num_strands` setting (`canvas-src/config/initializers/delayed_job.rb:78`);
  callers pass `n_strand: [...]` (`canvas-src/app/models/course.rb:609, :617, :652`).
  Name expansion is **[gem, unverified locally]**.
- **Singleton**: at most one running and one queued row per singleton, via two
  partial unique indexes (`20101216224513_create_delayed_jobs.rb:58-60` and
  `:73-76`); the insert trigger flips `next_in_strand := false` for the queued
  twin (`:116-121`); unlocking a running singleton deletes its queued twin
  (trigger at `:242`). Callers: `singleton: "recompute_student_scores:#{global_id}"`
  (`canvas-src/app/models/course.rb:1750`),
  `singleton: "horizon_content_discovery:#{global_id}"` (`:1719`).
- Stranded jobs with a long-delayed `run_at` are explicitly not relied upon:
  `stranded_run_at_grace_period = 1.year`
  (`canvas-src/config/initializers/delayed_job.rb:90`).

### 1.6 Worker process model and lifecycle settings

- Workers are separate processes from the web app, configured by
  `config/delayed_jobs.yml` (queue/count/max_priority) and driven by
  `Delayed::CLI` (`canvas-src/config/initializers/delayed_job.rb:84`).
- Batch/sleep: `fetch_batch_size = 20` (`:74`), `sleep_delay = 2.0` and
  `sleep_delay_stagger = 2.0` seconds (`:81-82`).
- Execution is wrapped in lifecycle hooks: around(:perform) (`:122`),
  before(:retry) (`:183`), before(:error) (`:210`); each run activates the job's
  shard and sets logging/error context (`:122-160`).
- Logging fields: detailed log = `tag, strand, singleton, priority, attempts,
  created_at, max_attempts, source, account_id` plus shard/jobs-cluster/
  db-cluster/root-account context (`canvas-src/config/initializers/delayed_job.rb:16-52`,
  wired at `:75-76`).
- Admin tooling for stuck work is manual: `PUT /jobs2/manage`,
  `PUT /jobs2/unstuck`, `GET /jobs2/stuck/strands|singletons`
  (`canvas-src/config/routes.rb:3072-3077`); manage takes the strand advisory
  lock and calls `JobsMigrator.unblock_strand!`
  (`canvas-src/app/controllers/jobs_v2_controller.rb:275-288`). Canvas does not
  auto-recover rows locked by a dead worker in any code visible here; health
  checks are configurable (`canvas-src/config/initializers/delayed_job.rb:84-85`).
  Star's lease-based recovery below is therefore Star-specific, not parity.

---

## 2. Star current state

### 2.1 Tables (star-db migrations)

- `jobs` — `migration/crates/star-db/migrations/0033_jobs.sql:18`: `tag`,
  `workflow_state` CHECK `'queued'|'running'|'completed'|'failed'|'cancelled'`
  (`:28-31`), `progress` 0..100 CHECK (`:30-33`), user/context columns,
  timestamps. The header already declares the contract: durable record only, no
  worker started by star-jobs (`0033_jobs.sql:2-7`).
- 0057 extends it with the Delayed::Job columns
  (`migration/crates/star-db/migrations/0057_jobs_v2.sql:14-30`): `strand` (:14),
  `singleton` (:15), `shard_id` (:16), `max_concurrent` default 1 (:17),
  `priority` (:18), `attempts` (:19), `max_attempts` default 1 (:20),
  `locked_by` (:21), `locked_at` (:22), `run_at` default now() (:23), `handler`
  (:24), `failed_at` (:25), `original_job_id` (:26), `requeued_job_id` (:27),
  `last_error` (:28), `next_in_strand` default TRUE (:29), `queue` (:30).
  Indexes cover bucket scans, strand, shard, original_job_id (`:34-43`).
- `job_clusters` (`0057_jobs_v2.sql:49-59`) — dashboard rows keyed by unique
  `database_server_id`.
- `delayed_jobs` (`migration/crates/star-db/migrations/0224_canvas_schema_tables_b.sql:30-53`)
  and `failed_jobs` (`:354-376`) exist as schema-parity copies of the Canvas DDL.
  Verified: no code in `star-jobs/src` or `star-graphql/src` reads or writes
  them (grep, no matches) — they are dead parity weight until an implementation
  decides otherwise (open question 5.1).

Schema gaps for a worker (named as TODOs, no SQL written here):
- a claim/lease marker — `jobs.claim_expires_at TIMESTAMPTZ` (recommended) or a
  policy of renewing `locked_at` (section 3.7);
- a cooperative-cancel marker — `jobs.cancel_requested BOOLEAN NOT NULL DEFAULT FALSE`
  (section 3.6);
- optional partial unique indexes backing singleton exclusion in the claim query
  (section 3.2).

### 2.2 Registered route surface

Mounted under `/api/v1`: `star_jobs::router()` merged in
`migration/crates/star-http/src/main.rs:189`; assembly in
`migration/crates/star-jobs/src/lib.rs:334-343`.

V1 JSON mode (`routes_v1.rs:398-403`):
- `GET /jobs`, `GET /jobs/{id}`, `POST /jobs/batch_update`.

V2 grouped (`routes_v2_grouped.rs:355-359`):
- `GET /jobs2/{bucket}/by_{group}`, `GET /jobs2/{bucket}/by_{group}/search`.

V2 manage (`routes_v2_manage.rs:360-367`):
- `PUT /jobs2/manage`, `PUT /jobs2/unstuck`, `GET /jobs2/throttle/check`,
  `PUT /jobs2/throttle`, `GET /jobs2/stuck/strands`, `GET /jobs2/stuck/singletons`.

V2 core (`lib.rs:339-341`):
- `GET /jobs2/clusters`, `GET /jobs2/{id}` (matches `original_job_id`),
  `POST /jobs2/{id}/requeue`.

Bucket predicates mirror Canvas (`lib.rs:154-167`): queued = `workflow_state =
'queued' AND run_at <= now() AND locked_by IS NULL AND locked_at IS NULL`;
running = `workflow_state = 'running' OR locked_at IS NOT NULL`; future = queued
with `run_at > now()`; failed = `workflow_state = 'failed' OR failed_at IS NOT
NULL`. All endpoints are admin-gated (`require_admin`, `lib.rs:317`); tests pin
`403`/`401` (`tests/jobs_v2_integration.rs:1367`).

**Correction to the task brief**: there is **no** `POST /jobs/:id/cancel`
registered anywhere in `migration/crates` (grep). The only cancel endpoint is
`POST /progress/{id}/cancel` (`migration/crates/star-auth/src/progress_jwts.rs:106-128`,
registered at `:211`), which flips `progresses.workflow_state` to `'failed'`
with an optional message for the owner or an admin. Cancel semantics for jobs
are a design proposal + open question (3.6, 5.2).

### 2.3 Enqueue-only, today

- `star_jobs::enqueue_job` inserts one `jobs` row, `workflow_state='queued'`,
  `progress=0` (`lib.rs:577-599`; INSERT at `:590`; doc comment "No worker is
  started and nothing is scheduled", `lib.rs:570-576`).
- `POST /jobs2/{id}/requeue` copies a failed row into a fresh queued row and
  stamps `requeued_job_id` (`lib.rs:521-557`; INSERT at `:538`).
- One out-of-crate enqueue path exists: `star-outcomes` keeps a private
  `enqueue_job` helper that inserts `jobs` rows with a strand
  (`migration/crates/star-outcomes/src/outcomes_api.rs:996-1003`, used at
  `:883-891` for `OutcomeImport::run` and `:2219`) — two enqueue
  implementations today (design 3.10 unifies them in the implementation PR).
- Verified absent (`grep migration/crates`): no worker loop, no claim/lock
  UPDATE on `locked_by`/`locked_at`, no `attempts`/`run_at` maintenance, no
  handler execution, no job cancel endpoint.

### 2.4 Missing for MIG-04a

1. claim + run (`FOR UPDATE SKIP LOCKED`);
2. handler registry + payload decode;
3. attempts / backoff / max_attempts;
4. failed-state flow (`failed_at`/`last_error`; today the V2 failed bucket can
   only match manually seeded rows);
5. runtime strand/singleton enforcement (`next_in_strand` maintenance);
6. cancel;
7. dead-worker recovery;
8. shutdown + structured logging;
9. one canonical enqueue entry point.

---

## 3. Design

### 3.1 Process model

- **`star-http --worker`**: the star-http binary gains a worker mode that starts
  N worker loops (default 1) alongside — or with a `--worker-only` toggle,
  instead of — the HTTP listeners. This is the mode CI drives ("runs as
  `star-http --worker` so CI executes enqueue → run → retry → recover",
  `full-migration-plan.md:96`).
- **Standalone loop**: the actual implementation is one async function,
  `star_jobs::worker::run_loop(pool, WorkerConfig)`, with no HTTP dependency, so
  a minimal standalone binary (or test harness) can run the same loop. No
  scheduling logic lives in star-http.
- `WorkerConfig`: `poll_interval` (default 2s — Canvas `sleep_delay`,
  `canvas-src/config/initializers/delayed_job.rb:81`), `lease_ttl`,
  `heartbeat_interval`, `batch_size` (default 1; Canvas fetches 20,
  `:74` — Star claims one row per iteration so strand fairness stays trivial),
  `worker_name` (`star-worker-{n}@{boot_id}`).
- Loop iteration: recover stale claims (3.7, cheap indexed scan) → claim one
  job (3.2) → decode payload (3.4) → run handler (3.3) → finish
  (success/retry/fail, 3.5). Idle → sleep `poll_interval`.

### 3.2 Claim protocol (FOR UPDATE SKIP LOCKED, strand-aware)

Design sketch (non-normative; no migration SQL in this change):

```sql
WITH eligible AS (
  SELECT id FROM jobs
  WHERE workflow_state = 'queued'
    AND run_at <= now()
    AND next_in_strand = TRUE
    AND (singleton IS NULL OR NOT EXISTS (
          SELECT 1 FROM jobs j2
          WHERE j2.singleton = jobs.singleton AND j2.id <> jobs.id
            AND (j2.workflow_state = 'running' OR j2.locked_by IS NOT NULL)))
  ORDER BY priority ASC, run_at ASC, id ASC
  LIMIT 1
  FOR UPDATE SKIP LOCKED
)
UPDATE jobs
SET workflow_state = 'running', locked_by = $worker, locked_at = now(),
    next_in_strand = FALSE, updated_at = now()
WHERE id IN (SELECT id FROM eligible)
RETURNING ...;
```

- Eligibility mirrors Canvas: due `run_at`, unlocked, `next_in_strand`
  (`20101216224513_create_delayed_jobs.rb:62-64`), and singleton exclusion
  (Canvas enforces it with two partial unique indexes, `:58-60`, `:73-76`;
  Star enforces it inside the claim predicate, with a schema TODO for an
  airtight partial unique index).
- `SKIP LOCKED` is the exclusion primitive: a second worker skips contended
  rows and takes the next eligible one, so two workers never double-run a row.
- **Strand-aware grouping**: the claim sets `next_in_strand = FALSE` on the
  claimed row (it now occupies a running slot). On finish, inside the same
  transaction, promote the next queued row(s) of that strand — ordered by
  `(strand_order_override, id)` ASC — up to `max_concurrent - still_running`
  slots. This reproduces the Canvas after-delete trigger
  (`20101216224513_create_delayed_jobs.rb:132-231`) without DB triggers on the
  `jobs` table. The update is scoped under a strand advisory lock (or a
  `SELECT ... FOR UPDATE` over the strand) to survive concurrent finishes.
- Priority: `(priority, run_at, id)` ASC, matching Canvas' claim index
  (`:62-64`). Confirmation of desired direction is open question 5.6.
- All of claim/finish/recovery is one transaction each; no lock is held across
  handler execution (the row is claimed by `locked_by`/`locked_at`, not by an
  open transaction).

### 3.3 Handler registry

- `trait JobHandler { fn tag(&self) -> &'static str; async fn run(&self, ctx:
  JobCtx, payload: &JobPayload) -> Result<JobOutcome, JobError>; fn
  max_attempts(&self) -> Option<u32> { None } }` — the handler's `tag` must
  equal the enqueued `jobs.tag`.
- Registry: a process-wide `tag -> Box<dyn JobHandler>` map built once
  (e.g. `OnceLock`), populated at startup by feature crates. First real
  entrant: `OutcomeImport::run` (strand already set by the caller,
  `star-outcomes/src/outcomes_api.rs:883-891`).
- `JobCtx` carries `job_id`, `user_id`, `context_type/context_id`, `shard_id`,
  and a progress sink writing `jobs.progress` (0..100, `0033_jobs.sql:30-33`)
  and the paired `progresses` row — Canvas couples job↔progress transitions
  queued→running→completed/failed in `Progress#process_job`
  (`canvas-src/app/models/progress.rb:124-137`).
- Unknown tag → **permanent failure** (attempts = max_attempts,
  `failed_at = now()`, `last_error = "unregistered job tag"`). Mirrors Canvas'
  discard behavior for jobs whose target vanished (`on_max_failures` for
  `Delayed::Backend::RecordNotFound`,
  `canvas-src/config/initializers/delayed_job.rb:113-117`): do not retry a job
  that can never succeed.

### 3.4 Payload decode

- Star-enqueued rows: `jobs.handler` holds a JSON document
  (`{"args": [...], "kwargs": {...}}`) written by the enqueue path; decode with
  `serde_json`. This deviates from Canvas' YAML-of-Ruby-object (§1.2) and is
  acceptable because every parity consumer treats `handler` as opaque text
  (the API emits it verbatim and never parses it — crate doc `lib.rs:8-13`).
- Foreign/legacy rows: a handler starting with the YAML marker `--- !ruby`
  (Canvas' shape; the test fixture uses `--- !ruby/object:Proc {}`,
  `lib.rs:647`) is not executable in Rust → permanent fail with
  `last_error = "non-JSON handler; Canvas YAML payload not executable"`.
  Never attempt YAML evaluation.
- Decode errors (unknown shapes, wrong arg types) are permanent failures, not
  retries — retrying cannot fix a malformed payload.

### 3.5 Retry, backoff, max_attempts (Canvas-compatible)

- On handler error: `attempts += 1`, `last_error = error`, clear
  `locked_by`/`locked_at`, keep `workflow_state = 'queued'`.
- If `attempts >= max_attempts`: `workflow_state = 'failed'`,
  `failed_at = now()` — this is what the V2 failed bucket already matches
  (`lib.rs:166`). `max_attempts` comes from `jobs.max_attempts`
  (default 1, `0057_jobs_v2.sql:20`), which equals Canvas' worker default
  (`canvas-src/config/initializers/delayed_job.rb:77`).
- Failed-row placement: keep the row in `jobs` (recommended; §5.1). If the
  coordinator wants the Canvas archive shape, the same transaction copies the
  row into `failed_jobs` with `original_job_id` set (Canvas DDL
  `20101216224513_create_delayed_jobs.rb:249-271`; semantics
  `canvas-src/app/controllers/jobs_v2_controller.rb:293-299`).
- Retry `run_at` = `now + (attempts^4 + 5)` seconds → attempts 1/2/3 give +6s,
  +21s, +86s. Canvas in-repo precedent:
  `canvas-src/lib/turnitin/outcome_response_processor.rb:85`,
  `canvas-src/lib/basic_lti/basic_outcomes.rb:269`. Implement as one const fn
  with a unit-table test; it is the single knob if the pinned gem disagrees
  (§5.5).
- Requeue semantics already match Canvas' "one more attempt past max_attempts":
  `POST /jobs2/{id}/requeue` resets `attempts = 0` on the new row
  (`lib.rs:545`; Canvas comment
  `canvas-src/app/controllers/jobs_v2_controller.rb:293-298`).

### 3.6 Cancel

Validated correction: `POST /jobs/:id/cancel` is **not** registered (§2.2).
Proposed design, matching the registered progress-cancel authorization pattern:

- Route (to add in the implementation PR): `POST /jobs/{id}/cancel`, allowed for
  the job's `user_id` owner or an admin — same rule as
  `POST /progress/{id}/cancel` (`star-auth/src/progress_jwts.rs:106-119`).
- Queued/future job: `workflow_state = 'cancelled'` (`0033_jobs.sql:28-31`
  allows it), `next_in_strand = FALSE` so the strand slot frees, claim query
  excludes it. The handler must never run.
- Running job: cooperative. Needs a schema TODO column
  `jobs.cancel_requested BOOLEAN NOT NULL DEFAULT FALSE`; handlers receive a
  cancellation token and check it at await points; on observe, finalize as
  `'cancelled'` with `last_error = cancel message` (mirroring the progress
  cancel's `message` parameter, `progress_jwts.rs:119-128`). Terminal rows:
  cancel is an idempotent no-op returning current state.
- Paired `progresses` row: mark failed + message, exactly as the registered
  progress cancel does (`progress_jwts.rs:119-128`).

### 3.7 Recovery of rows left running by a dead worker

- Every claim sets `locked_by`/`locked_at`. A `running` row whose lease is
  stale is recoverable. Two options:
  - **(a, recommended)** TODO(schema): add `jobs.claim_expires_at TIMESTAMPTZ`,
    set at claim and renewed by a heartbeat (default 30s, TTL 5×). Recovery =
    indexed scan for `workflow_state='running' AND claim_expires_at < now()`.
    Keeps `locked_at` semantically identical to Canvas (set once at lock time).
  - **(b, no schema change)** reuse `locked_at` as a renewed lease (worker
    updates it each heartbeat). Deviation: Canvas never re-touches `locked_at`
    mid-run, so the running bucket's ordering column changes meaning.
- Recovery action (one transaction, same bookkeeping as a failure): set
  `workflow_state='queued'`, `attempts += 1` (the attempt did happen),
  `last_error = "recovered: lease of <locked_by> expired"`, `run_at = now()`
  (+ backoff if the attempt limit is now reached → straight to `'failed'`),
  clear `locked_by`/`locked_at`/`claim_expires_at`, re-evaluate
  `next_in_strand` for the strand.
- Startup sweep: each worker runs the recovery scan on start and periodically
  (cheap indexed predicate) before claiming. Two workers racing a recovery are
  serialized by `FOR UPDATE` in the recovery transaction.
- No SQL is written in this spec; the column decision is open question 5.3.

### 3.8 Shutdown

- `SIGTERM`/`SIGINT`: stop claiming; let the in-flight handler finish (or
  observe `cancel_requested`, 3.6); the normal finish path releases the claim;
  exit 0. This is the "finish current, release claim" contract from
  `full-migration-plan.md:96`.
- Second signal: abandon the loop and rely on lease-based recovery (3.7) —
  never clear the claim of a still-running handler, or the job double-runs.
- CI harness: `--worker` mode with `poll_interval=100ms` runs until quiet, then
  exits, enabling enqueue→run→retry→recover assertions.

### 3.9 Logging

- Structured `tracing` JSON per lifecycle event: `claimed`, `started`,
  `progress`, `succeeded`, `failed(will_retry, next_run_at)`,
  `failed_final(failed_at)`, `cancelled`, `recovered(from_worker)`.
- Fields mirror Canvas' detailed log — `tag, strand, singleton, priority,
  attempts, max_attempts, run_at, source, shard_id`, plus worker name, duration,
  error (`canvas-src/config/initializers/delayed_job.rb:47-52`, wired `:75-76`;
  Canvas also logs cluster/shard/root-account context, `:16-26`).
- `locked_by` = `star-worker-{n}@{boot_id}` so admins can attribute running rows
  (Canvas uses a procname prefix, `canvas-src/config/initializers/delayed_job.rb:83`).
- Payload text is never logged (parity: Canvas logs tag/singleton context, not
  handler bodies).

### 3.10 Enqueue path unification

- `star_jobs::enqueue_job` grows optional `strand`, `singleton`,
  `max_attempts`, `priority`, `run_at`, and JSON `handler` parameters and
  becomes the single enqueue entry point; `star-outcomes`' private INSERT
  (`outcomes_api.rs:996-1003`) moves onto it in the implementation PR (not this
  doc) so tags and strands stay consistent with the registry.

---

## 4. Test plan (integration; `migration/crates/star-jobs/tests`)

New file `tests/jobs_worker_integration.rs`, following the existing
`TestServer` + `PgPool` setup pattern (`tests/jobs_new_integration.rs:33-110`:
`setup()` returns `Option` and skips cleanly without `DATABASE_URL`). CI-only
execution per workspace guardrails (no local cargo test).

| # | Case | Assertions |
|---|---|---|
| 4.1 | `enqueue_runs_exactly_once` | enqueue via `star_jobs::enqueue_job` + registered no-op handler; loop until quiet → handler ran exactly once (execution row), `workflow_state='completed'`, `progress=100`, claim cleared, V2 queued-bucket scan empty afterwards |
| 4.2 | `retry_to_limit_backoff` | handler failing until `attempts=3`, row seeded `max_attempts=3`: after failure 1 → queued, `attempts=1`, `run_at ≈ now+6s`; failure 2 → `+21s`; failure 3 → `workflow_state='failed'`, `failed_at` set, `last_error` recorded, V2 failed bucket lists it; then requeue → queued copy, `attempts=0`, `requeued_job_id` stamped (`lib.rs:521-557`) |
| 4.3 | `kill_recover` | seed `running` row with `locked_by='dead-worker'` and stale lease; start loop → recovered to `'queued'` with `attempts=1` and recovery `last_error`, then executes; strand slot freed and next stranded row proceeds; no double execution |
| 4.4 | `cancel` | queued: cancel → `'cancelled'`, handler never invoked, claim query skips it; running (variant gated on the `cancel_requested` column): handler observes token → final `'cancelled'` with message, paired `progresses` row failed |
| 4.5 | `strand_ordering` | 3 rows one strand, `max_concurrent=1`, equal priority, ids increasing + one `strand_order_override`; single worker executes strictly in `(strand_order_override, id)` order, never two concurrently (timestamps), `next_in_strand` promoted after each finish |
| 4.6 | `two_workers_never_double_run` | two concurrent loops over 20 seeded jobs incl. a singleton pair: every tag runs exactly its row count (20 total executions), singleton strictly serial, no concurrent duplicate execution |
| 4.7 | unit: backoff table | plain `#[test]` for `attempts^4+5` values and the `attempts == max_attempts` boundary (fail vs retry), next to the const fn (no DB) |

---

## 5. Open questions for the coordinator

1. **Canonical table.** Spec proposes `jobs` (the registered API surface reads
   it; it has `workflow_state`/`progress`; `0033_jobs.sql:2-7` already promises a
   worker advances it) with `delayed_jobs`/`failed_jobs` remaining
   schema-parity-only. Plan row 1.5 says "choose the canonical table (`jobs` or
   `delayed_jobs`, both exist) from the backend each Canvas caller uses"
   (`full-migration-plan.md:96`) — confirm `jobs`.
2. **Cancel route.** Brief expected a registered `POST /jobs/:id/cancel`; it is
   not registered (§2.2). Approve adding it per 3.6, or restrict cancellation to
   progress rows (`POST /progress/{id}/cancel` already exists,
   `star-auth/src/progress_jwts.rs:211`)?
3. **Recovery lease.** Add `jobs.claim_expires_at` (recommended, preserves
   `locked_at` parity) vs renew `locked_at` as the lease (no schema change,
   deviates from Canvas semantics). Schema TODOs are named only — no SQL in this
   change. A partial unique index on singleton (Canvas has two,
   `20101216224513_create_delayed_jobs.rb:58-60, :73-76`) is a second, smaller
   schema TODO.
4. **Payload format.** JSON in `jobs.handler` (documented deviation from Canvas
   YAML) — confirm parity consumers can treat `handler` as opaque, and that
   YAML rows failing fast as unregistered is the desired interop behavior.
5. **Backoff formula source.** `attempts^4 + 5` is pinned from Canvas' in-repo
   retry code (`lib/turnitin/outcome_response_processor.rb:85`,
   `lib/basic_lti/basic_outcomes.rb:269`); the worker-level reschedule lives in
   `switchman-inst-jobs` 4.3.2 (`canvas-src/Gemfile.d/app.rb:29`,
   `canvas-src/Gemfile.lock:1204`), whose source is not vendored in
   `canvas-src/` and could not be verified in this run. Confirm against the
   pinned gem before the implementation freezes the constant.
6. **Priority direction and pool shape.** Canvas orders `(priority, run_at, id)`
   ASC (`20101216224513_create_delayed_jobs.rb:62-64`) and runs priority-banded
   pools (`canvas-src/config/delayed_jobs.yml:2-7`). Star proposes a single
   pool, lower-number-first. Confirm banding is out of scope for MIG-04a.
7. **Worker count in `--worker` mode.** Default 1 for CI determinism vs six
   production workers in `canvas-src/config/delayed_jobs.yml:2-7` — is a
   workers-count knob required this phase?
