# UI-shell research base (epic: ui-shell)

Research artifacts for the server-rendered UI shell (MIG-06) and the nine
UI journey issues. Generated 2026-11-20; star-lms side verified against
`main` @ `a403ee2`, Canvas side against a sparse checkout of
`instructure/canvas-lms` master @ `1c9f0bb` (app/views, app/controllers,
app/helpers, app/models, config/routes.rb).

| File | Contents |
|---|---|
| `repo-integration-guide.md` | star-lms integration facts for the upcoming `star-ui` crate: exact APIs to reuse (star-auth, star-common, star-api, star-dashboard, star-courses, star-pages, star-modules, star-enrollments, star-users, star-announcements), the integration-test template, and the CI wiring checklist for a new crate (shard maps, Cargo.lock via the CI-assist workflow, parity caveats). File:line referenced. |
| `canvas-reference.md` | Per-page Canvas reference for all ten UI issues: Rails routes, templates, DOM structure (ids/classes), data sources, forms, permission gates; which REST endpoints each page needs and whether star-lms implements them; per-issue porting recommendations (essential vs skippable for a no-JS server-rendered first pass). |

Issue mapping: #43/#44/#112 shell + first journey + js_env harness
(authoritative contract: `../p5-ui-shell.md`); #70 login/auth; #71
assignments + gradebook; #72 discussions + inbox; #73 settings/admin;
#108 SpeedGrader; #109 quiz-taking; #110 files; #111
announcements/syllabus/calendar.

These are research artifacts, not specs. The authoritative contract
remains `specs/p5-ui-shell.md`; deviations discovered during
implementation belong in the implementing PR, not as edits here.
