# star-ui frontend — Svelte 5 + TypeScript, static bundle

The Canvas-reusing front end. Markup and class names come from Canvas; the
stylesheet comes from Canvas; what is new here is the compile step and the type
gate.

## Shape

```
migration/ui/
  src/lib/api.ts          same-origin /api/v1 client + the endpoint map
  src/lib/auth.svelte.ts  the session (Svelte 5 module state)
  src/lib/token.ts        where the Bearer token comes from
  src/lib/env.ts          typed reader over window.ENV (the Rust contract)
  src/lib/types.ts        shapes = Canvas serializer keys
  src/styles/tokens.css   --ic-* custom properties, cited to Canvas
  src/styles/canvas/      compiled Canvas bundles (npm run styles:canvas)
  src/routes/             one directory per Canvas page
  scripts/build-canvas-css.sh
```

`MIGRATION.md` is the ordered plan for retiring the Askama templates. Read it
before deleting anything — there is a parity constraint that sequences the work.

## Measured, 2026-11-24 (`npm run build`, adapter-static)

| Quantity | Value |
|---|---|
| First load (shell + every preloaded chunk + CSS), raw | 112,052 B |
| First load, **brotli -q11** | **36,974 B** |
| First load, gzip -9 | 41,866 B |
| All route chunks together, raw / gzip | 139,478 B / 50,595 B |
| CSS authored here (tokens + base only, no Canvas bundle yet) | 3,480 B raw / 1,163 B gz |
| Fallback shell HTML | 2,126 B |
| Build time | 3.31 s |
| Type gate | `svelte-check` 0 errors, 0 warnings |
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
