#!/usr/bin/env bash
# Regenerate src/styles/canvas/*.css from Canvas' own SCSS bundles.
#
# Why a script and not a dependency: star-labs does not vendor Canvas source,
# so this cannot run in CI. It runs on a workstation that has a canvas-src
# checkout (with `yarn install` done, because Canvas' base/environment.scss
# imports the `sass-direction` npm package for direction-sides()), and its
# output is a committed artifact reviewed like any other vendored file.
#
# Usage:
#   CANVAS_SRC=~/lms-migration/canvas-src npm run styles:canvas
#
# The point of this file is parity: the class names our components emit are
# Canvas', and this is the only stylesheet allowed to define them. Adding a
# hand-written rule for an .ic-* or .Button-* selector is a drift bug.
set -euo pipefail

CANVAS_SRC="${CANVAS_SRC:?set CANVAS_SRC to a canvas-src checkout}"
OUT="$(cd "$(dirname "$0")/.." && pwd)/src/styles/canvas"

# One entry per page we ship, named exactly as Canvas names its bundle
# (app/stylesheets/bundles/, 139 of them). Canvas loads one of these per page,
# not all of them, which is why the first payload stays small.
BUNDLES=(
  common
  dashboard
  dashboard_card
  all_courses
  course_list
  assignments
  assignments_2_teacher
  assignments_2_student
  canvas_quizzes
  discussion_topics
  wiki_pages
  calendar2
  grades
  login
)

test -d "$CANVAS_SRC/app/stylesheets/bundles" || {
  echo "CANVAS_SRC=$CANVAS_SRC has no app/stylesheets/bundles" >&2
  exit 1
}
test -d "$CANVAS_SRC/node_modules/sass-direction" || {
  echo "missing sass-direction — run \`yarn install\` in $CANVAS_SRC first" >&2
  exit 1
}

mkdir -p "$OUT"
for bundle in "${BUNDLES[@]}"; do
  src="$CANVAS_SRC/app/stylesheets/bundles/${bundle}.scss"
  [ -f "$src" ] || { echo "skip ${bundle} (no such bundle)" >&2; continue; }
  npx --yes sass \
    --load-path="$CANVAS_SRC/app/stylesheets" \
    --no-source-map \
    --style=compressed \
    "$src" "$OUT/${bundle}.css"
  echo "built ${bundle}.css ($(wc -c <"$OUT/${bundle}.css") bytes)"
done

cat <<'EOF'
NOTE: the --ic-* custom properties Canvas' rules reference are normally emitted
by lib/brandable_css.rb at build time. Until that generated sheet is vendored
too, src/styles/tokens.css is the stand-in: same names, values copied from
app/stylesheets/base/_variables.scss with line citations.

NOTE: high contrast is a separate variant upstream ($use_high_contrast). This
script builds the normal-contrast variant only; a port that claims pixel parity
without the high_contrast variant is incomplete, so the gap is tracked in
MIGRATION.md rather than papered over by the root-attribute token overrides.
EOF
