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


## Step 1 — Make the API reachable from the bundle (pick one)

`/api/v1` authenticates with `Authorization: Bearer` only. star-auth's
`AuthUser` extractor reads that header and has no cookie path, while the web
shell authenticates with the `star_session` cookie. The SPA needs one of:

- **1a. Emit the token in ENV.** Add `api_token` to `JsEnv` and `ENV_KEYS` in
  `crates/star-ui/src/env.rs`. Cheapest, and `env.ts`/`token.ts` already read it.
  Cost: the JWT becomes readable by any XSS on the page, for the whole token
  lifetime (`TOKEN_EXPIRY_SECS`), because `last_logged_out` does not bound a
  token the page already handed out.
- **1b. (preferred) Accept the cookie on `/api/v1`.** Teach `AuthUser` to fall
  back to the `star_session` cookie and require the `star_csrf` token on unsafe
  methods — the mechanism PR #247 already built for the web forms. Keeps the
  credential out of JS; costs one extractor change plus CSRF coverage.

The interim path in this branch (sessionStorage after `/auth/login`) works
without either, but a token in `sessionStorage` is XSS-readable for the tab's
lifetime, so it is a bridge, not the end state. **Do not ship 1a and the bridge
together.**

## Step 2 — Serve the bundle from axum

- `crates/star-ui/src/lib.rs`: mount the built files
  (`migration/ui/build` → `crates/star-ui/static/ui/`) via
  `ServeDir`, and point every page route's handler at one `shell` function that
  renders a single `shell.html` template: the existing `_head` contract with
  `window.ENV` inline, plus `<script type="module" src="/ui/…">`.
- Add a `fallback_to_index` for client-side routes that have no Canvas URL (the
  SPA's own deep links). Canvas-real paths must keep explicit registrations —
  that is the parity constraint above.
- `adapter-static` emits `build/index.html` as the fallback document. Cache it at
  `max-age=0, must-revalidate` and hash-named assets at
  `max-age=31536000, immutable`; that asymmetry is the whole point of the static
  choice — under load, the shell is one small uncached HTML response and
  everything else is a cache hit.
- `static/canvas/` and `static/fonts/` ship inside the same `ServeDir` mount, at
  `/canvas/…` and `/fonts/…` — the exact paths Canvas' compiled rules already
  contain, so no sheet needs rewriting. Serve them `immutable` like the JS.
- Two things `src/lib/canvasStyles.ts` does in the browser today belong in the
  shell once axum renders it, and moving them is what makes the first paint
  Canvas-correct rather than Canvas-after-a-frame:
  1. the `<link>` tags (tokens → fonts → common → page bundle, per
     `ENV.use_high_contrast`) — the reason it is JS now is that a Vite bundle
     cannot pick a variant at request time, but `shell.html` can;
  2. `body` classes (`primary-nav-expanded`, or `full-width ic-Login-Body` on the
     auth pages), which Canvas computes in `application.html.erb:83-110`.
  `src/app.css` then loses its three declarations too, because they exist only to
  cover the window before those tags exist.


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

- `styles:canvas` builds the normal-contrast variant only. Canvas ships eight
  (`config/brandable_css.yml`); the high-contrast token overrides are in
  `tokens.css`, but variant-specific rules are not captured yet.
- No bundle has been built, so bundle size is unmeasured — the resource claims in
  `README.md` are architectural, not measured.
- Nothing here is verified in a browser yet. `npm run check` is the gate, and CI
  does not run it today; wiring it into `rust-migration.yml` is a coordinator
  change, not a worker one.
- `courses/[id]/pages` (wiki) and `modules` are not ported; the tab component
  renders whatever the tabs API returns, so those tabs link to routes the SPA
  does not have yet.
