# Styles

One rule: **we do not author styles for Canvas' class names.**

| File | What it is | Written by |
|---|---|---|
| `tokens.css` | The `--ic-*` custom properties Canvas' sheets reference, copied from `canvas-src/app/stylesheets/base/_variables.scss` and `brandable_variables.json`, each with its line number | us, as a stand-in for the sheet `lib/brandable_css.rb` generates |
| `canvas/*.css` | Compiled Canvas bundles (`app/stylesheets/bundles/<name>.scss`), one per page we ship | `npm run styles:canvas` — committed output, never hand-edited |

## Why not Tailwind

Tailwind can express any look, but expressing *this* look means rewriting ~2.2 MB
of Canvas SCSS by hand and hoping the measurements match. Two failures make it
unusable for pixel parity:

1. **Preflight.** Tailwind's reset changes box-sizing, border defaults, margins
   and font smoothing globally. Canvas' sheets assume *their* reset
   (`components/_ic-reset.scss`), so importing Tailwind moves Canvas' own
   numbers.
2. **Proven drift.** Our previous hand-ported palette had six values that did not
   match Canvas (`#0374b5` vs `#2B7ABC`, `#2d3b45` vs `#34444f`, `4px` vs `6px`
   radius, and a font stack missing `"Lato Extended"`). Nothing caught it,
   because nothing diffed them. A transcription with citations can be diffed; a
   re-expression cannot.

## Known gap

`styles:canvas` builds the **normal-contrast** variant only. Canvas ships eight
variants (`config/brandable_css.yml`: normal/high-contrast × LTR/RTL ×
dyslexic). `tokens.css` covers the high-contrast *token* overrides, but not the
rules that variant compiles differently. Tracked in `../MIGRATION.md`.

## Fonts

`$ic-font-family` leads with `"Lato Extended"`, Instructure's own cut. It is not
in `canvas-src/public/fonts` (that directory has `canvas-icons` only), so the
woff2 files must be vendored from the upstream font package before this is
honestly pixel-perfect; metric fallback to `Lato` is the current state.
