#!/usr/bin/env bash
# Copy exactly the assets the compiled Canvas CSS asks for, out of
# CANVAS_SRC/public and into this package's static/ tree.
#
# Why parse the CSS instead of copying canvas-src/public/fonts: that directory is
# 16 MB across 247 files, most of them formats no current browser requests (.eot,
# .svg fallbacks) or font families this deployment never selects. canvas-src/public
# is ground truth for *what exists*; the compiled bundles are ground truth for *what
# is referenced*. This script vendors the intersection, so the static dir stays
# honest and small, and a rule that starts referencing a new file makes the next run
# copy it rather than silently 404.
#
# Usage:
#   CANVAS_SRC=~/lms-migration/canvas-src npm run assets:canvas
set -euo pipefail

CANVAS_SRC="${CANVAS_SRC:?set CANVAS_SRC to a canvas-src checkout}"
UI="$(cd "$(dirname "$0")/.." && pwd)"
CSS_DIR="$UI/static/canvas"
STATIC="$UI/static"

[ -d "$CSS_DIR/normal_contrast" ] || {
  echo "no compiled bundles in $CSS_DIR — run npm run styles:canvas first" >&2
  exit 1
}

# url("/fonts/x.woff2"), url('/images/y.png') and url(/images/z.gif) are all forms
# Canvas and our generated token sheet use, so the quote character is optional on
# both sides. Getting this wrong is invisible at build time and shows up as a
# missing brand logo on the running page.
paths=$(grep -rhoE "url\(['\"]?(/[^'\")]+)['\"]?\)" "$CSS_DIR" \
  | sed -E "s/^url\(['\"]?//; s/['\"]?\)$//" \
  | sort -u)

# Canvas' own broken references, kept broken. Each is a file the vendored SCSS
# asks for that does not exist in canvas-src/public, verified 2026-11-24:
#
#   /fonts/open_dyslexic/OpenDyslexic-BoldItalic.woff2
#     bundles/fonts.scss:41 declares the face as 'open_dyslexic/OpenDyslexic-BoldItalic'
#     but the directory holds OpenDyslexic-Bold-Italic.woff2 (extra hyphen). Canvas
#     404s this and the browser synthesises the oblique from OpenDyslexic-Bold.
#     Renaming the file here would make our dyslexic-font mode render differently
#     from Canvas, which is the opposite of the point of this directory.
KNOWN_MISSING=(
  "/fonts/open_dyslexic/OpenDyslexic-BoldItalic.woff2"
  # static/canvas/*/quizzes.css emits both. Neither name appears anywhere in
  # canvas-src/app/stylesheets (grep 'canvas@2x' returns nothing), so they arrive
  # through a sass helper this checkout does not define, and canvas-src/public
  # has no copy. The rule is a question-image placeholder in
  # app/stylesheets/pages/quizzes/_quizzes.scss, so a missing one costs a
  # decorative background on one page — not a metric, as it would for a font.
  "/images/canvas.png"
  "/images/canvas@2x.png"
)

copied=0
missing=0
while IFS= read -r p; do
  [ -n "$p" ] || continue
  case "$p" in
    /*) ;;
    *) continue ;;
  esac
  src="$CANVAS_SRC/public$p"
  if [ ! -f "$src" ]; then
    for known in "${KNOWN_MISSING[@]}"; do
      [ "$p" = "$known" ] && continue 2
    done
    echo "MISSING $p (referenced by CSS, absent from $CANVAS_SRC/public)" >&2
    missing=$((missing + 1))
    continue
  fi
  dst="$STATIC$p"
  mkdir -p "$(dirname "$dst")"
  # Only copy when absent or byte-different, so reruns do not rewrite 5 MB.
  if [ ! -f "$dst" ] || ! cmp -s "$src" "$dst"; then
    cp "$src" "$dst"
    copied=$((copied + 1))
  fi
done <<< "$paths"

echo "copied $copied asset file(s), $missing missing, into $STATIC"
if [ "$missing" -ne 0 ]; then
  cat <<'EOF' >&2

A referenced file is not in canvas-src/public. Either the bundle needs an asset
Canvas builds at deploy time, or our bundle list pulls in a page we do not ship.
Do not stub it: a missing font changes every line height on the page.
EOF
  exit 1
fi
