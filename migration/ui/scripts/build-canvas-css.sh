#!/usr/bin/env bash
# Regenerate static/canvas/<variant>/*.css from Canvas' own SCSS bundles.
#
# Why a script and not a dependency: star-labs does not vendor Canvas source, so
# this cannot run in CI. It runs on a workstation with a canvas-src checkout, and
# its output is a committed artifact reviewed like any other vendored file.
#
# Usage:
#   CANVAS_SRC=~/lms-migration/canvas-src npm run styles:canvas
#
# Verified working against canvas-src on 2026-11-24: dart-sass 1.83.4 emits real
# Canvas rules (`--ic-badge{background:var(--ic-brand-primary)}`,
# `#03893d`, `#e62429`) for all 29 bundles in both variants. Three prerequisites:
#
#   1. CANVAS_SRC/node_modules must hold the packages some bundles @import by
#      relative path. Canvas gets them from a full `yarn install`; here one
#      copy per package is enough (see scripts/canvas-sass-deps.txt):
#        npm install --prefix /tmp/csd <pkg>@<version>
#        cp -R /tmp/csd/node_modules/<pkg> "$CANVAS_SRC/node_modules/"
#      `sass-direction` (base/_environment.scss:32) and `emoji-mart@3.0.1`
#      (components/_emoji.scss:18 — assignments, assignments_2_student,
#      gradebook) are both checked below.
#   2. The variant directory first on the load path. `base/_variables.scss:34`
#      imports "variant_variables", which lives at
#      app/stylesheets/variants/new_styles_normal_contrast/_variant_variables.scss.
#      Without that --load-path nothing resolves. Its position ahead of
#      app/stylesheets matters: that is how the variant overrides base partials.
#   3. public/javascripts on the load path — a few component partials reach
#      into the JS tree for a scss variable.
#
# The point of this file is parity: the class names our components emit are
# Canvas', and this is the only place those rules may come from. Adding a
# hand-written rule for an .ic-* or .Button-* selector is a drift bug.
set -euo pipefail

CANVAS_SRC="${CANVAS_SRC:?set CANVAS_SRC to a canvas-src checkout}"
OUT="$(cd "$(dirname "$0")/.." && pwd)/static/canvas"
SASS_VERSION="${SASS_VERSION:-1.83.4}"

# One entry per page we ship, named exactly as Canvas names its bundle
# (app/stylesheets/bundles/ holds 139). Canvas loads one per page, not all 139,
# which is what keeps a first visit small. `calendar2` is intentionally absent:
# it @imports fullcalendar from node_modules and needs the same treatment as
# sass-direction if the calendar page goes SPA-side.
BUNDLES=(
  fonts
  common
  dashboard
  dashboard_card
  all_courses
  course_list
  discussions
  discussions_index
  discussions_edit
  announcements_index
  assignments
  assignments_2_teacher
  assignments_2_student
  assignments_edit
  canvas_quizzes
  quizzes
  grade_summary
  gradebook
  user_grades
  wiki_page
  pages
  syllabus
  login
  login_confirm
  registration
  profile_show
  profile_edit
  course_settings
  conversations_new
  user_list_boxes
  user_logins
)

VARIANTS=(normal_contrast high_contrast)

test -d "$CANVAS_SRC/app/stylesheets/bundles" || {
  echo "CANVAS_SRC=$CANVAS_SRC has no app/stylesheets/bundles" >&2
  exit 1
}
test -f "$CANVAS_SRC/node_modules/sass-direction/_direction.scss" || {
  echo "missing node_modules/sass-direction in $CANVAS_SRC — see the header of this script" >&2
  exit 1
}
test -f "$CANVAS_SRC/node_modules/emoji-mart/css/emoji-mart.css" || {
  echo "missing node_modules/emoji-mart in $CANVAS_SRC (assignments, assignments_2_student and gradebook import it) — see scripts/canvas-sass-deps.txt" >&2
  exit 1
}

built=0
failed=0
# sass is a devDependency of this package, so use the local binary rather than
# paying an npx resolution per bundle (58 compiles).
SASS_BIN="$(cd "$(dirname "$0")/.." && pwd)/node_modules/.bin/sass"
[ -x "$SASS_BIN" ] || SASS_BIN="npx --yes sass@${SASS_VERSION}"
for variant in "${VARIANTS[@]}"; do
  out_dir="$OUT/$variant"
  mkdir -p "$out_dir"
  for bundle in "${BUNDLES[@]}"; do
    src="$CANVAS_SRC/app/stylesheets/bundles/${bundle}.scss"
    [ -f "$src" ] || { echo "skip ${bundle} (no such bundle)" >&2; continue; }
    if "$SASS_BIN" \
      --no-source-map --style=compressed \
      --quiet --quiet-deps \
      --load-path="$CANVAS_SRC/app/stylesheets/variants/new_styles_${variant}" \
      --load-path="$CANVAS_SRC/app/stylesheets" \
      --load-path="$CANVAS_SRC/public/javascripts" \
      "$src" "$out_dir/${bundle}.css" 2>/dev/null; then
      built=$((built + 1))
    else
      echo "FAILED ${variant}/${bundle}" >&2
      failed=$((failed + 1))
    fi
  done
done

echo "built $built css files, $failed failures -> $OUT"

# Fonts are data, not CSS: fonts.scss emits url("/fonts/lato/extended/…woff2")
# and the woff2 files are in canvas-src/public/fonts. Serve them at the same
# /fonts/ path axum serves the bundle, or the metric fallback silently changes
# every line height. Copy them with:
#   mkdir -p ../../crates/star-ui/static/fonts
#   cp -R "$CANVAS_SRC/public/fonts/"* ../../crates/star-ui/static/fonts/
cat <<'EOF'

Reminders:
- tokens.css stands in for the sheet lib/brandable_css.rb generates. Once the
  generated brandable sheet is vendored, it replaces tokens.css rather than
  sitting beside it.
- Both normal_contrast and high_contrast variants are built here, because
  ENV.use_high_contrast selects between them at request time.
EOF
