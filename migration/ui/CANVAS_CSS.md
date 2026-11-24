# Where the CSS comes from

One rule: **we do not author styles for Canvas' class names.** Every `.ic-*`,
`.Button-*`, `.ui-tabs*` rule a page needs is compiled out of `canvas-src` and
committed as generated output. A hand-written rule for one of those selectors is a
drift bug, because Canvas' version of the same selector is also in the tree and
which one wins becomes an accident of load order.

| Path | What it is | Written by |
|---|---|---|
| `static/canvas/<variant>/*.css` | Canvas' own bundles (`app/stylesheets/bundles/<name>.scss`), one per page we ship, for `normal_contrast` and `high_contrast` | `npm run styles:canvas` |
| `static/canvas/<variant>/tokens.css` | The brandable custom properties the bundles read | `npm run tokens:canvas` |
| `static/fonts/`, `static/images/` | Exactly the files those sheets reference by `url()` | `npm run assets:canvas` |
| `src/app.css` | Three declarations: the font family and page background to use for the frames before the `<link>`s land | us |

All three generators read `CANVAS_SRC` (a canvas-src checkout) and are idempotent;
nothing in `static/canvas` is edited by hand. The dependency list the SCSS needs
inside `CANVAS_SRC/node_modules` is in `scripts/canvas-sass-deps.txt`.

## Why not Tailwind

Tailwind can express any look, but expressing *this* look means rewriting ~2.2 MB of
Canvas SCSS by hand and hoping the measurements match. Two failures make it unusable
for pixel parity:

1. **Preflight.** Tailwind's reset changes box-sizing, border defaults, margins and
   font smoothing globally. Canvas' sheets assume *their* reset
   (`components/_ic-reset.scss`), so importing Tailwind moves Canvas' own numbers.
2. **Proven drift.** The hand-ported palette this replaced had six values that did not
   match Canvas (`#0374b5` vs `#2B7ABC`, a `#2d3b45` header vs `#334451`, 4px radius
   vs 6px, and a font stack missing `"Lato Extended"`). Nothing caught it, because
   nothing diffed them.

## How this differs from Canvas' build

- Canvas generates `tokens.css` in Ruby (`lib/brandable_css.rb:168-210`,
  `computed_variables` + `all_brand_variable_values_as_css`). The Node port of those
  43 lines of math reproduces it, including `CanvasColor`'s channel-wise
  darken/lighten with integer truncation (`gems/canvas_color/lib/canvas_color.rb:328,346`),
  which is why `--ic-brand-font-color-dark-lightened-15` is `#47535C` and not the
  `#2D3D4A` you get by adding 15 % to HSL lightness instead.
- Image values keep their `/images/` prefix. Canvas emits `url('canvas_logomark_only@2x.png')`
  and lets the Rails asset host resolve it; under a plain static root that resolves
  relative to the sheet and 404s, so we emit the absolute path.
- `loadCanvasStyles()` appends at run time what Canvas emits server-side from
  `layouts/_head.html.erb`, because the variant is a per-user choice and a Vite
  bundle is a per-build one. MIGRATION.md step 2 moves these tags into axum's shell.

## Known gaps

- **Two of eight variants.** `config/brandable_css.yml` ships normal/high-contrast ×
  LTR/RTL × dyslexic. We build the two LTR normal-branch sheets. RTL needs
  `variants/new_styles_normal_contrast_rtl` on the load path instead — one flag, not
  a redesign, and it is untested here.
- **`calendar2` is not built.** It `@import`s fullcalendar from node_modules; the
  calendar page still runs on the vendored base sheet plus `common.css`.
- **Canvas' own 404s are reproduced.** `bundles/fonts.scss:41` asks for
  `open_dyslexic/OpenDyslexic-BoldItalic.woff2`, but the directory holds
  `OpenDyslexic-Bold-Italic.woff2`. Upstream is broken, so in dyslexic mode a bold
  italic is synthesised. Fixing the filename here would make us differ from Canvas.
  Same story for `/images/canvas.png` and `/images/canvas@2x.png`, which
  `quizzes.css` references and `canvas-src/public` does not contain; both are listed
  in `scripts/vendor-canvas-assets.sh`.

## Fonts

`$ic-font-family` (`base/_variables.scss:213`) leads with `"Lato Extended"`. The five
woff2 weights are in `canvas-src/public/fonts/lato/extended/` and are vendored by
`npm run assets:canvas`, and `fonts.css` carries Canvas' own `@font-face` block
pointing at `/fonts/lato/extended/…`. So the real face loads — at 182 KB per weight,
which is the first-load cost the README measures. Subsetting is the fix and it is a
decision, not a default: it changes no glyph Canvas renders but it does change the
files.
