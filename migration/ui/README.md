# star-ui frontend — Svelte 5 + TypeScript, static bundle

The Canvas-reusing front end. Markup and class names come from Canvas; the
stylesheet comes from Canvas; what is new here is the compile step and the type
gate.

## Shape

```
migration/ui/
  src/lib/api.ts           same-origin /api/v1 client + the endpoint map
  src/lib/auth.svelte.ts   the session (Svelte 5 module state)
  src/lib/token.ts         no credential to hold: ENV token, else the cookie
  src/lib/env.ts           typed reader over window.ENV (the Rust contract)
  src/lib/canvasStyles.ts  appends Canvas' <link>s, in Canvas' order
  src/lib/types.ts         shapes = Canvas serializer keys
  src/lib/icons/           Canvas' own nav SVGs, copied from app/views/shared/svg
  src/app.css              3 declarations — the pre-paint floor, nothing else
  static/canvas/<variant>/ Canvas' compiled bundles (npm run styles:canvas)
  static/fonts/, static/images/  exactly the assets that CSS references (npm run assets:canvas)
  build/                   adapter-static output; index.html + _app are committed
  scripts/                 build-canvas-css.sh, build-brandable-tokens.mjs,
                           vendor-canvas-assets.sh, canvas-sass-deps.txt

crates/star-ui/            the serving side of the same contract
  build.rs                 embeds build/ + static/, hashes each file
  embed/shell-map.txt      route → (Canvas bundles, body classes)
  src/shell.rs             injects title/ENV/links/classes into index.html
  src/assets.rs            serves the embedded files, with the cache families
  src/session_bridge.rs    star_session → Authorization: Bearer on /api/v1
```

There is no hand-written stylesheet in this package apart from `src/app.css`.
`static/canvas/` is generated from `canvas-src/app/stylesheets` and `static/fonts`
and `static/images` are copied by parsing those sheets for `url()`, so an asset
appears here only when Canvas' CSS actually asks for it.

`MIGRATION.md` is the ordered plan for retiring the Askama templates. Read it
before deleting anything — there is a parity constraint that sequences the work.

## Measured, 2026-11-24 (`npm run build`, adapter-static, brotli -q11)

The dashboard route, normal contrast, cold cache. Numbers are the files a browser
actually fetches: `build/index.html`, the 21 preloaded chunks in its
`<link rel=modulepreload>`, the five sheets `loadCanvasStyles()` appends
(tokens, fonts, common, dashboard, dashboard_card) and the two Lato weights the
first text paints with.

| Quantity | raw | brotli | gzip |
|---|---|---|---|
| JS — shell, chunks, entry, route node 0 | 117,215 | 42,707 | 47,952 |
| Canvas CSS — the dashboard stack | 416,051 | 54,774 | 68,581 |
| Lato Extended Regular + Bold (woff2, Canvas' files) | 367,620 | 359,520 | 360,773 |
| Fallback shell HTML | 2,197 | 502 | 658 |
| **Cold first load** | **903,083** | **457,503** | **477,964** |
| First load without the two fonts | 535,463 | 97,983 | 117,191 |
| All 15 route nodes together | 38,943 | 16,076 | 18,448 |
| One extra page's sheet, e.g. assignments.css | 30,410 | 5,822 | — |

Read the rows this way: **the fonts are the payload**, not the framework. Lato
Extended ships every Latin codepoint Canvas has ever rendered, 182 KB per weight,
and woff2 is already compressed so brotli takes 182 KB to 179 KB. `common.css` —
Canvas' own 400 KB base sheet, 51 KB compressed — is second. Both are immutable and
paid once per client for the life of the release; navigating to a new page after
that costs a route node (~1 KB) and one page bundle.

`common.css` at 51 KB brotli is not this port being wasteful: it is the same file
Canvas serves, from the same SCSS. Cutting it means tree-shaking Canvas' sheet,
which is exactly the drift the user asked to avoid. The honest optimisation is
subsetting Lato to the glyphs in use (a `pyftsubset` pass would take the 360 KB to
roughly 30 KB and change no pixel), and that is a decision to make with the fonts
visible, not in a PR.

| Other | Value |
|---|---|
| Canvas bundles compiled per variant | 31, plus the generated token sheet = 32 files |
| Vendored files in `static/` | 236 files, 4,160,228 B (fonts 2,181,616; sheets 1,533,493; images 444,072) |
| Embedded into the binary (`build.rs`) | 277 files, 4,310,494 B, of which `build/_app` is 151,055 B (55,087 B gzipped) |
| Build time | 2.81 s |
| Type gate | `svelte-check` 0 errors, 0 warnings |
| Unit tests | 15 passed (`npm run test:unit`) |


Resource profile, which is the reason for the shape:

- **No second process, and no second artifact.** `crates/star-ui/build.rs` embeds
  `build/index.html`, `build/_app` and `static/{canvas,fonts,images}` into the
  binary with `include_bytes!`; `assets::serve_embedded` answers them from memory.
  Deployment is one file — no repo checkout, no `ServeDir` path to mount, no Node
  runtime to run, patch, health-check or scale. The only per-request work on the
  Rust side is `shell::render`, which injects the title, `window.ENV`, the
  stylesheet links and the body classes into the document SvelteKit emitted.
- **Cacheable everything.** `/_app/immutable/**` (Vite-hashed) and
  `/canvas/{sha256-prefix}/{variant}/{name}.css` (hashed by `build.rs`, because
  Canvas' sheet names are fixed) are `max-age=31536000, immutable`. Canvas' fonts
  and images keep their literal paths — its own sheets reference them that way —
  so they get `max-age=86400, must-revalidate` and an `ETag`. Only the ~2 KB
  shell is uncached, so a returning visitor costs one small HTML fetch plus API
  JSON: the UI itself never leaves the browser cache.
- **Per-page CSS, like Canvas.** `crates/star-ui/embed/shell-map.txt` is the
  server-side route → bundle table (mirrored by each page's
  `loadCanvasStyles([...])` call), so the dashboard does not download gradebook
  rules, and a typo in it fails the build rather than shipping a 404 `<link>`.
- **The session is a cookie, not a token.** `crates/star-ui/src/session_bridge.rs`
  turns `star_session` into the bearer header `AuthUser` verifies, guarded by the
  same-origin check and the `star_csrf` double-submit Rails uses. No credential is
  ever readable by script. See step 1 of `MIGRATION.md`.

## Commands

```bash
npm install
npm run check        # svelte-check — the gate for fleet-authored pages
npm run test:unit    # vitest
npm run build        # -> build/, embedded into star-ui by crates/star-ui/build.rs
npm run dev          # vite dev on :5173, proxy target is the axum origin
```

## Rules for adding a page

1. **Rebuild and commit `build/` with the source.** `crates/star-ui/build.rs`
   embeds `build/index.html` and `build/_app/**` (see `migration/ui/.gitignore`),
   and the Rust CI job has no Node step, so the committed bundle *is* what the
   binary serves. A `.svelte` change without `npm run build` looks like a hydration
   bug in CI, because that is literally what ships.
2. If the page needs Canvas bundles other than tokens/fonts/common, add its path to
   `crates/star-ui/embed/shell-map.txt` — the build fails if a named bundle has no
   compiled sheet, which is the point.
3. Paths live in `src/lib/api.ts` only, written exactly as Canvas' `routes.rb`
   serves them. Never inline a fetch path in a `.svelte` file — that is how
   invented verbs get shipped, and the parity scoreboard counts them.
4. Field names in `src/lib/types.ts` are Canvas serializer keys. Renaming one is
   a parity break, not a style choice.
5. Class names are Canvas'. If a needed class has no rule, it comes from
   `styles:canvas`, not from a new hand-written block (see
   `src/styles/README.md` for why).
6. `npm run check` before pushing. With `ssr = false`, a wrong prop type is a
   runtime blank page, and CI does not run this yet.
7. Role gating reads `session.isTeacher`, which is the role the server reported —
   never a claim parsed out of the JWT.
