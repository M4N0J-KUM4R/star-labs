# star-ui frontend — Svelte 5 + TypeScript, static bundle

The Canvas-reusing front end. Markup and class names come from Canvas; the
stylesheet comes from Canvas; what is new here is the compile step and the type
gate.

## Shape

```
migration/ui/
  src/lib/api.ts           same-origin /api/v1 client + the endpoint map
  src/lib/auth.svelte.ts   the session (Svelte 5 module state)
  src/lib/token.ts         where the Bearer token comes from
  src/lib/env.ts           typed reader over window.ENV (the Rust contract)
  src/lib/canvasStyles.ts  appends Canvas' <link>s, in Canvas' order
  src/lib/types.ts         shapes = Canvas serializer keys
  src/lib/icons/           Canvas' own nav SVGs, copied from app/views/shared/svg
  src/app.css              3 declarations — the pre-paint floor, nothing else
  static/canvas/<variant>/ Canvas' compiled bundles (npm run styles:canvas)
  static/fonts/, static/images/  exactly the assets that CSS references (npm run assets:canvas)
  scripts/                 build-canvas-css.sh, build-brandable-tokens.mjs,
                           vendor-canvas-assets.sh, canvas-sass-deps.txt
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
| Vendored files in `static/` | 236, 4.6 MB (fonts 2.1 MB, images 892 KB, sheets 1.4 MB) |
| Build time | 2.81 s |
| Type gate | `svelte-check` 0 errors, 1 warning (Canvas' own `href="#"`) |
| Unit tests | 7 passed |


Resource profile, which is the reason for the shape:

- **No second process.** `build/` is files; axum serves them. There is no Node
  runtime to run, patch, health-check, or scale, and no per-request render cost
  on the Rust side beyond the one shell template that injects `window.ENV`.
- **Cacheable everything.** Hash-named chunks under `/_app/immutable/` are
  `max-age=31536000, immutable`; only the ~2 KB shell is revalidated. For a large
  user base the marginal cost of a returning visitor is one small HTML fetch plus
  API JSON — the UI itself never leaves the browser cache.
- **Per-page CSS, like Canvas.** A page imports the Canvas bundles it renders, so
  the dashboard does not download gradebook rules.

## Commands

```bash
npm install
npm run check        # svelte-check — the gate for fleet-authored pages
npm run test:unit    # vitest
npm run build        # -> build/, copied to crates/star-ui/static/ui/
npm run dev          # vite dev on :5173, proxy target is the axum origin
```

## Rules for adding a page

1. Paths live in `src/lib/api.ts` only, written exactly as Canvas' `routes.rb`
   serves them. Never inline a fetch path in a `.svelte` file — that is how
   invented verbs get shipped, and the parity scoreboard counts them.
2. Field names in `src/lib/types.ts` are Canvas serializer keys. Renaming one is
   a parity break, not a style choice.
3. Class names are Canvas'. If a needed class has no rule, it comes from
   `styles:canvas`, not from a new hand-written block (see
   `src/styles/README.md` for why).
4. `npm run check` before pushing. With `ssr = false`, a wrong prop type is a
   runtime blank page, and CI does not run this yet.
5. Role gating reads `session.isTeacher`, which is the role the server reported —
   never a claim parsed out of the JWT.
