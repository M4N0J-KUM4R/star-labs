# Replacing the Askama UI with the Svelte bundle

The old server-rendered UI is 36 templates in `crates/star-ui/templates/` plus one
hand-ported stylesheet (`crates/star-ui/static/app.css`). This is the order to
remove them. It is not a delete-first list: step 1 and 2 are what make the
deletion safe.

## The constraint that sequences everything else

The parity scoreboard counts **route registrations** in `crates/*/src/*.rs`. REST
registration parity is 100 % (1410/1410) and **86 of the 160 "extras" are Canvas
web routes** — the HTML pages star-ui registers. Delete a `get(handlers::…)`
line and the scoreboard drops.

So the swap is: **keep every route, replace every handler body with one shell
render, then delete the templates.** The URLs stay identical to Canvas' (which is
what parity measures) while the markup becomes the SPA's.

## Step 0 — Styles and fonts (done on this branch)

Pixel parity is a stylesheet question, not a framework question. It is answered by
`CANVAS_CSS.md`; the rest of this plan assumes it.

1. `npm run styles:canvas` compiles Canvas' own bundles
   (`canvas-src/app/stylesheets/bundles/*.scss`) to
   `migration/ui/static/canvas/<variant>/<bundle>.css` — 31 bundles ×
   `normal_contrast`/`high_contrast`, 62 files, 0 failures as of 2026-11-24.
   Requires a canvas-src checkout holding `sass-direction` and `emoji-mart@3.0.1`
   in `node_modules` (`scripts/canvas-sass-deps.txt`), and the variant directory
   first on the load path or `base/_variables.scss:34` cannot resolve
   `variant_variables`.
2. `npm run tokens:canvas` **generates** the brandable sheet from
   `canvas-src/app/stylesheets/brandable_variables.json` plus the derivations in
   `lib/brandable_css.rb:168-210`, reproducing `CanvasColor`'s math. It is not a
   transcription, and it ends with a self-check that fails the build if a bundle
   reads a custom property no sheet defines.
3. `npm run assets:canvas` copies only the files those sheets reference: 236 files,
   including the five **Lato Extended** woff2 weights, which *are* in this checkout
   at `public/fonts/lato/extended/` (an earlier note here claimed otherwise, and
   was wrong). Canvas' own unresolvable references are listed, with citations, in
   `scripts/vendor-canvas-assets.sh` — reproduced rather than fixed.
4. Tailwind is deliberately absent. Its preflight resets box-sizing, borders,
   margins and font smoothing, which moves Canvas' own measurements; and our
   previous hand-ported palette proved the drift risk — six values did not match
   Canvas (`#0374b5` vs `#2B7ABC`, `#2d3b45` vs the real `#334451`, radius `4px`
   vs `6px`, and a font stack missing `"Lato Extended"`).


## Step 1 — Make the API reachable from the bundle (done: 1b)

`/api/v1` authenticated with `Authorization: Bearer` only. star-auth's `AuthUser`
extractor reads that header and has no cookie path, while the web shell
authenticates with the `star_session` cookie.

**1b landed, in a different place than first sketched.** `AuthUser` was *not*
taught a cookie branch — verification stays single-sourced in one header. Instead
`crates/star-ui/src/session_bridge.rs` is a middleware on the `/api/v1` nest
(`crates/star-http/src/main.rs`) that rewrites the session cookie into the bearer
header `AuthUser` already verifies. Two Rails-equivalent guards close the hole a
cookie-carried credential opens:

1. `Origin`, else `Referer`, must name the request host (`Origin: null` is
   rejected, not guessed).
2. Unsafe methods must send `X-CSRF-Token` equal to the non-HttpOnly `star_csrf`
   cookie — the same double-submit `session::check_csrf` compares for the shell's
   own forms. `src/lib/api.ts` does this on every write.

A request that fails either answers **403**, and an existing `Authorization`
header short-circuits the layer entirely, so the CLI, LTI tools and every
golden-response test behave exactly as before.

1a (emit `api_token` in ENV) was rejected: it makes the JWT readable by any XSS
for the whole `TOKEN_EXPIRY_SECS`, and `last_logged_out` cannot bound a token the
page already handed out. `env.ts` keeps the field only so a deployment *can*
supply one; nothing in the shell writes it.

The sessionStorage bridge is deleted. What remains for the auth pages:
`POST /api/v1/auth/login` does not set `star_session` (only the shell's
`POST /login` does — star-auth never names the cookie), so when `/login` itself
moves onto the shell it must submit that form rather than fetch the API. No new
route for this: Canvas has no cookie-granting API endpoint, and parity would
count it as an invention.

## Step 2 — Serve the bundle from axum (done, for `/profile`)

- `crates/star-ui/build.rs` embeds `migration/ui/build/{index.html,_app}` and
  `migration/ui/static/{canvas,fonts,images}` (+ favicons) into the binary as
  `include_bytes!`, each with a 32-bit sha256 revision. Embedding rather than
  `ServeDir` is what keeps spec §2.3 true — one binary, no filesystem dependency
  at runtime, no Node process — which also means the Rust CI job needs no
  `setup-node` step: **`build/index.html` and `build/_app` are committed** (see
  `migration/ui/.gitignore`) and must be rebuilt before a commit.
- `crates/star-ui/src/assets.rs::serve_embedded` is the router's **fallback**, not
  a route: `migration/tools/parity/parity.py` counts `.route(...)` calls, and
  `/fonts/**` has no business in the route table. A miss answers with a bare 404
  so an unmatched `/api/v1` path stays byte-identical to axum's default.
- Cache policy is per family: `/_app/**` (Vite-hashed) and
  `/canvas/{rev}/{variant}/{name}.css` get `max-age=31536000, immutable`; the
  unversioned forms — Canvas' own font and image paths, which its sheets
  reference by name and therefore cannot be renamed — get
  `max-age=86400, must-revalidate` plus an `ETag`, so a repeat view is a 304.
- `crates/star-ui/src/shell.rs` injects the per-request parts into the document
  `adapter-static` emitted: `<title>`, `window.ENV` (with `</` escaped so a
  course name cannot close the script block), the Canvas `<link>`s in Canvas'
  order, and the `<html>`/`<body>` attributes. `embed/shell-map.txt` is the
  route → (bundles, body classes) table, and a typo there is a **build failure**,
  because a 404 stylesheet reaches the browser as "the dashboard looks slightly
  wrong".
- `GET /profile` (config/routes.rb:1045, `ProfileController#show`) is the first
  handler on this path. It also ensures the `star_csrf` cookie, which the
  bundle's first write needs.
- `fallback_to_index` is deliberately **not** used: an SPA fallback would swallow
  every unmatched URL and turn a 404 into a 200, which is both a parity
  regression and a worse answer to a typo. Canvas-real paths keep explicit
  registrations; a client-side deep link the server does not know is a bug to fix
  in the route table.


## Step 3 — Port pages journey by journey

Order matches the journeys already proven end-to-end in Rust. For each page:
port markup to a `.svelte` route using Canvas' class names, add only the
endpoints the page needs to `src/lib/api.ts`, run `npm run check`, then delete
the Askama template it replaced.

| Lane | Canvas pages | Askama templates retired |
|---|---|---|
| auth | `login`, `register`, `forgot_password`, `reset_password` | 4 |
| dashboard | `dashboard`, `course_home`, `speedgrader*` | 4 |
| courses | `course_settings`, `course_people*`, `course_tab` | 4 |
| assignments | `assignments_index`, `assignment_form`, `assignment_show` | 3 |
| quizzes | `quizzes`, `quiz_show`, `quiz_edit`, `quiz_take`, `quiz_results` | 5 |
| discussions | `discussions_index`, `discussion_new/show`, `announcement*` | 5 |
| conversations | `conversations_index`, `conversation_compose/show` | 3 |
| files | `files_index`, `file_show`, `syllabus` | 3 |
| calendar/grades | `calendar`, `grades`, `gradebook` | 3 |
| misc | `error`, `layouts/` | 2 |

## Step 4 — Delete the old layer

After every page has a Svelte route:

1. `crates/star-ui/templates/` — all 36 files, and the `Template` structs in
   `src/handlers/*.rs` that only existed to feed them.
2. `crates/star-ui/static/app.css` — superseded by the vendored Canvas bundles.
   Do not port its values: they are the six-wrong-number set from step 0.
3. `askama` from `crates/star-ui/Cargo.toml`, replaced by one `shell.html` —
   the shell template survives, because something server-side must inject
   `window.ENV` exactly once.
4. `js-env-coverage.md` and the `ENV_KEYS` governance test (issue #112) stay —
   the SPA reads the same contract, so the test becomes more important, not less.

## Known gaps in this branch

- Only `/profile` renders through `shell.rs`. The other ten routes still return
  their Askama page, because each one has markup assertions in
  `tests/ui_integration.rs`, `tests/ui_journeys.rs` and `tests/auth_integration.rs`
  (~375 of them across the set) that describe the server-rendered DOM; converting
  a page means rewriting its assertions in the same commit, and that cannot be
  validated locally (cargo is CI-only). `embed/shell-map.txt` already carries all
  of them, so each conversion is one handler body plus its tests.
- `/courses` is **not** registered. Canvas' nav hardcodes `href="/courses"`
  (`_new_nav_header.html.erb:167`) and `CoursesController#index` exists
  (`courses_controller.rb:530`), but `config/routes.rb` has no web `get "courses"`
  row — so serving it would add a star route Canvas does not have, and
  `parity.yml` fails on a coverage change. The SPA's `/courses` remains reachable
  as a client-side route only. Task #30 is the classification fix that decides
  this properly.
- `styles:canvas` builds two variants. Canvas ships eight
  (`config/brandable_css.yml`); the high-contrast token overrides are in
  `tokens.css`, and `shell-map.txt` refuses a bundle that lacks either of the two
  compiled sets, but the RTL and dyslexic-specific rules are not captured. The
  dyslexic root attribute also needs
  `ENV.PREFERENCES.user_system_default_font_family`, which `EnvPreferences` does
  not carry yet — so `handlers::profile` passes `dyslexic: false`.
- Nothing is precompressed. Measured on this branch's build, in bytes: the
  embedded set is 277 files / 4,310,494 B; a `/profile` first load is 425,130 B
  of CSS (70,064 B gzipped: `common.css` alone is 64,948 of that) plus 151,055 B
  of JS (55,087 B gzipped), and the shell document itself is ~2 KB `no-store`.
  Serving `Content-Encoding: gzip` from the binary needs `flate2`,
  which is not in `Cargo.lock` — and CI builds `--locked`, so that is a lockfile
  change to land on `main` first, not something this branch can smuggle in.
  Until then compression is the reverse proxy's job.
- The browser check is on this branch: `browser-smoke/tests/profile-shell-styles`
  asserts the 84 px nav rail, the revisioned `common.css` URL and its `immutable`
  + 304 behaviour, `Lato Extended` as the computed font family, and that
  `star_session` stays out of `document.cookie` while a cookie-only
  `/api/v1/auth/me` returns 200. What it cannot cover is the ten pages still on
  Askama, and `npm run check` / `npm run test:unit` are still not in CI — they
  were run by hand for this branch (0 errors, 15 passed).
