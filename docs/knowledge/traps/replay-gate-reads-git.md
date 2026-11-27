---
tags: [trap, ci, parity, replay]
---
# Trap: the parity gate has to read the committed report out of git

**Status: FIXED** in `.github/workflows/canvas-replay.yml`.

`rust-anything app replay` **overwrites** `rust/app-cases/replay-report.md`
with the report of the run that just happened. The gate then compared

```sh
compare_replay.py --committed rust/app-cases/replay-report.md \
                  --fresh    rust/app-cases/replay-report.md
```

— the same file, twice. The comparison always agreed, so the gate could
never trip: a port that quietly lost a case would have kept CI green. A
gate that cannot fail is worse than no gate, because it is read as a gate.

**The fix.** The committed baseline is read out of git, where the checkout
put it and the replay cannot touch it:

```sh
git show HEAD:migration/rust/app-cases/replay-report.md > /tmp/committed-replay-report.md
python3 tools/parity/compare_replay.py \
  --committed /tmp/committed-replay-report.md \
  --fresh rust/app-cases/replay-report.md
```

The path in `git show` is relative to the **repository root**, not to the
job's working directory (`migration`).

**A crashed replay must not look like a pass.** The replay step removes the
report before it runs (`rm -f rust/app-cases/replay-report.md`), so a replay
that dies leaves no report, and the gate reports a broken run instead of
comparing the committed report with itself.

**What the gate compares.** The *set of failing cases*, not the prose: the
diff of a case shows the bodies, and the bodies depend on the rows of the
database the run had. A case that passes in the committed report and fails
now is a regression. A case that fails in the committed report and passes
now is an improvement, and the committed report has to be updated to record
it — otherwise the next run compares against a stale contract and a real
regression hides behind it. Both directions fail the job; only the set
matters.

**The failing set is a baseline, not a target.** The port does not answer
every recorded case correctly yet, so the committed report names the cases
that still fail. The job fails when that set *changes*, not when it is
non-empty — a job that demands 100% parity cannot be made green by any
amount of work short of finishing the port.
