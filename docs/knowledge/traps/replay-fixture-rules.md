---
tags: [trap, harness]
---
# Trap: the replay fixture must match the state the corpus was recorded in

**The corpus was recorded over several sessions with several tokens**, so the
database state it encodes is specific. Getting it wrong makes every case
answer 401 or 404 and the whole comparison meaningless.

The rules the recording encodes:

- **course 1 belongs to account 6**, not account 1. Every account-scoped rule
  Canvas resolves through `Account#courses` misses otherwise.
- **the course has a section**, or the `/sections/1/...` routes answer 404
  where Canvas answers 403.
- **the parity user is a teacher of course 1 and NOT an account admin.** The
  recorded `GET /api/v1/accounts` answers `[]` — that is the proof. Do not
  add `account_users` rows for it; the corpus is the contract even when the
  live Canvas now disagrees.
- **the enrollment is concluded mid-recording**, by the
  `DELETE /courses/1/enrollments/1` case. That is why the later course-scoped
  reads still answer 200 in Canvas but would 403 without a concluded
  enrollment.

**Two recorded rows are internally inconsistent** — the same file id answers
404 on GET and 403 on PUT — because they come from different recording
sessions. The corpus is the contract; a port cannot satisfy both.

**Related.** [[canvas-permission-order]], [[corpus-token-placeholder]].
