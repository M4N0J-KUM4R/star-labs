#!/usr/bin/env python3
"""Tests for golden_differ (Phase 2 shape parity, issue #231).

Each test pins one rule stated in ``golden_differ``'s module docstring. When a
test and the docstring disagree, the Canvas source cited in the golden decides:
run the serializer, do not edit whichever is more convenient.
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import golden_differ as gd  # noqa: E402

ACCOUNT_SOURCE = "canvas-src/lib/api/v1/account.rb:38 (account_json read_only branch)"

# The read_only branch of account_json returns exactly these six attributes plus
# default_time_zone; the storage-quota methods and every sis_*/include[] field
# belong to the non-read_only branch and are never emitted by course_accounts.
ACCOUNT_SHAPE = {
    "id": "integer",
    "name": {"$nullable": "string"},
    "parent_account_id": {"$nullable": "integer"},
    # root_account_id is NOT NULL in the schema but account_json assigns nil for
    # root accounts, so the serializer -- not the column -- sets nullability.
    "root_account_id": {"$nullable": "integer"},
    "workflow_state": "string",
    "uuid": {"$nullable": "string"},
    "default_time_zone": "string",
}

LIST_SHAPE = {"$list": ACCOUNT_SHAPE}

FULL_RECORD = {
    "id": 7,
    "name": "Root Account",
    "parent_account_id": None,
    "root_account_id": None,
    "workflow_state": "active",
    "uuid": "abcdef0123456789",
    "default_time_zone": "America/Denver",
}


def write_json(path: Path, doc) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(doc))
    return path


def golden_file(dir_: Path, route="GET /api/v1/course_accounts", shape=LIST_SHAPE,
                name=None, source=ACCOUNT_SOURCE):
    return write_json(dir_ / (name or gd.golden_filename(*route.split(" "))),
                      {"route": route, "source": source, "shape": shape})


def capture_file(dir_: Path, body, route="GET /api/v1/course_accounts", name=None):
    return write_json(dir_ / (name or gd.golden_filename(*route.split(" "))),
                      {"route": route, "body": body})


def test_golden_filename_matches_harness_path_handling():
    """Canvas' own path spellings must land on the file the harness writes."""
    # The manifest already carries /api/v1; a routes.rb-style path does not, and
    # canvas_base_path adds the prefix -- both must produce one name.
    assert gd.golden_filename("GET", "/api/v1/course_accounts") == \
        "GET__api_v1_course_accounts.json"
    assert gd.golden_filename("GET", "/course_accounts") == \
        "GET__api_v1_course_accounts.json"
    # Rails ':id' and our '{courseId}' are the same slot, so they must collide.
    assert gd.golden_filename("GET", "/api/v1/courses/:id") == \
        gd.golden_filename("GET", "/api/v1/courses/{courseId}") == \
        "GET__api_v1_courses_param.json"
    # An optional segment is not part of the registered path.
    assert gd.golden_filename("GET", "/api/v1/users/:id(/:scope)") == \
        "GET__api_v1_users_param.json"
    # Root-mounted route sets keep their own prefix.
    assert gd.golden_filename("POST", "/api/lti/authorizations") == \
        "POST__api_lti_authorizations.json"


def test_field_miss_is_actionable():
    shape = {"id": "integer", "name": {"$nullable": "string"}}
    findings = []
    gd.diff_shape("GET /x", shape, {"id": 1}, "", findings)
    # 'name' is required to be present (nullable means "may be null", not "may
    # be absent"), so its absence is a FieldMiss.
    miss = [f for f in findings if f.kind == "FieldMiss"]
    assert [f.path for f in miss] == ["name"]
    assert gd.is_actionable(miss[0]) is True


def test_type_mismatch_is_actionable_and_boolean_is_not_integer():
    findings = []
    gd.diff_shape("GET /x", {"id": "integer"}, {"id": "7"}, "", findings)
    bad = [f for f in findings if f.kind == "TypeMismatch"]
    assert len(bad) == 1 and bad[0].path == "id"
    assert gd.is_actionable(bad[0]) is True

    # Python bool subclasses int; a boolean must not satisfy an integer slot.
    findings = []
    gd.diff_shape("GET /x", {"id": "integer"}, {"id": True}, "", findings)
    assert [f.kind for f in findings] == ["TypeMismatch"]


def test_unexpected_null_is_actionable_unless_declared_nullable():
    findings = []
    gd.diff_shape("GET /x", {"workflow_state": "string"},
                  {"workflow_state": None}, "", findings)
    nulls = [f for f in findings if f.kind == "UnexpectedNull"]
    assert len(nulls) == 1 and gd.is_actionable(nulls[0]) is True

    # The same key wrapped $nullable (root_account_id) is Canvas behaviour, not
    # a defect, so no finding at all.
    findings = []
    gd.diff_shape("GET /x", {"root_account_id": {"$nullable": "integer"}},
                  {"root_account_id": None}, "", findings)
    assert findings == []


def test_extra_field_is_reported_but_gates_only_when_strict():
    findings = []
    gd.diff_shape("GET /x", {"id": "integer"}, {"id": 1, "surprise": "here"},
                  "", findings)
    extra = [f for f in findings if f.kind == "ExtraField"]
    assert len(extra) == 1
    assert gd.is_actionable(extra[0]) is False
    assert gd.is_actionable(extra[0], strict_extras=True) is True


def test_empty_list_is_not_a_silent_pass(tmp_path):
    """An empty array proves nothing about element shape.

    course_accounts is caller-scoped, so an anonymous capture is [] by design
    (canvas-src/app/controllers/accounts_controller.rb:491). The differ must
    report that as an evidence gap rather than crediting it as shape parity.
    """
    canvas, captured = tmp_path / "canvas", tmp_path / "captured"
    golden_file(canvas)
    capture_file(captured, [])

    corpus = gd.load_corpus(canvas, captured, None)
    findings = gd.compare(corpus)
    gaps = [f for f in findings if f.kind == "NoElements"]
    assert len(gaps) == 1
    # The gap is reported at the list position, so the finding points at the
    # element that would have carried the shape.
    assert gaps[0].path == ""
    # Not actionable by default (nothing is known to be wrong yet)...
    assert gd.is_actionable(gaps[0]) is False
    # ...but a corpus-wide verdict refuses to call an empty capture a pass.
    assert gd.is_actionable(gaps[0], require_capture=True) is True

    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured)]) == 0
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured), "--require-capture"]) == 1


def test_include_reuses_another_goldens_shape(tmp_path):
    """``$include`` makes one golden the single definition of a body shape.

    ``GET /api/v1/courses`` and ``PUT /api/v1/courses/:id`` both return the
    ``course_json`` body (canvas-src/lib/api/v1/course.rb:73-75, :106-116), so
    the index and the update must not maintain two copies that can drift.
    """
    canvas, captured = tmp_path / "canvas", tmp_path / "captured"
    single = "GET__api_v1_courses_param.json"
    golden_file(canvas, shape=ACCOUNT_SHAPE, name=single,
                route="GET /api/v1/courses/:id")
    golden_file(canvas, shape={"$list": {"$include": single}},
                route="GET /api/v1/courses")

    capture_file(captured, [FULL_RECORD], route="GET /api/v1/courses")
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured)]) == 0

    # A key required by the *included* golden is still a miss on the outer one,
    # reported at the captured element so the finding names this route.
    broken = dict(FULL_RECORD)
    del broken["uuid"]
    capture_file(captured, [broken], route="GET /api/v1/courses")
    findings = gd.compare(gd.load_corpus(canvas, captured, None))
    misses = [f for f in findings if f.kind == "FieldMiss"]
    assert [f.path for f in misses] == ["[0].uuid"]
    assert misses[0].route == "GET /api/v1/courses"


def test_include_cycle_and_unknown_target_are_corpus_errors(tmp_path):
    """An unresolvable include is exit 2, never a silently empty comparison."""
    canvas = tmp_path / "canvas"
    golden_file(canvas, shape={"$include": "GET__api_v1_courses.json"},
                name="GET__api_v1_courses_param.json",
                route="GET /api/v1/courses/:id")
    golden_file(canvas, shape={"$include": "GET__api_v1_courses_param.json"},
                name="GET__api_v1_courses.json", route="GET /api/v1/courses")
    assert gd.main(["selfcheck", "--canvas-dir", str(canvas)]) == 2

    dangling = tmp_path / "dangling"
    golden_file(dangling, shape={"$include": "GET__never_written.json"},
                name="GET__api_v1_courses.json", route="GET /api/v1/courses")
    assert gd.main(["selfcheck", "--canvas-dir", str(dangling)]) == 2


def test_committed_canvas_corpus_passes_selfcheck():
    """The goldens shipped in this repo are the ones the gate actually reads."""
    root = Path(__file__).resolve().parents[2]
    canvas = root / "parity" / "behavior" / "golden" / "canvas"
    assert canvas.is_dir(), f"no canvas golden corpus under {root}"
    assert gd.main(["selfcheck", "--canvas-dir", str(canvas)]) == 0


def test_selfcheck_validates_citations_and_rejects_bad_goldens(tmp_path):
    canvas = tmp_path / "canvas"
    golden_file(canvas)
    assert gd.main(["selfcheck", "--canvas-dir", str(canvas)]) == 0

    # A golden without a Canvas citation is an opinion, not truth: refuse it.
    golden_file(canvas, shape={"id": "integer"}, name="GET__uncited.json", source="")
    assert gd.main(["selfcheck", "--canvas-dir", str(canvas)]) == 2

    # A shape node outside the grammar is a malformed golden.
    bad = tmp_path / "bad"
    golden_file(bad, shape={"id": {"nested": "not_a_token"}}, name="GET__bad.json")
    assert gd.main(["selfcheck", "--canvas-dir", str(bad)]) == 2


def test_exit_status_clean_actionable_and_accepted_deviation(tmp_path):
    canvas, captured = tmp_path / "canvas", tmp_path / "captured"
    deviations = tmp_path / "deviations.json"

    golden_file(canvas)
    capture_file(captured, [FULL_RECORD])
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured)]) == 0

    # Drop a field Canvas always sends: actionable, exit 1.
    broken = dict(FULL_RECORD)
    del broken["workflow_state"]
    broken["id"] = "seven"
    capture_file(captured, [broken])
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured)]) == 1

    # Record it as accepted against Canvas source and the gate goes quiet. The
    # field is a shape position, not the index this particular capture landed on.
    route = "GET /api/v1/course_accounts"
    write_json(deviations, {"deviations": [
        {"route": route, "field": "[*].workflow_state", "class": "FieldMiss",
         "reason": "star omits workflow_state pending #231 seeded capture",
         "source": ACCOUNT_SOURCE},
        {"route": route, "field": "[*].id", "class": "TypeMismatch",
         "reason": "id serialized as string", "source": ACCOUNT_SOURCE},
    ]})
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured),
                    "--deviations", str(deviations)]) == 0

    # Position-independence is the reason for the spelling: a capture that
    # returns more records must not invent a fresh, unaccepted finding.
    capture_file(captured, [broken, broken, broken])
    assert gd.main(["diff", "--canvas-dir", str(canvas),
                    "--capture-dir", str(captured),
                    "--deviations", str(deviations)]) == 0

    # An acceptance is scoped: it must not silence a different route. Canvas
    # goldens and captures share a file name (that is the point of the
    # flattening), so they live in separate directories.
    other_canvas, other_captured = tmp_path / "oc", tmp_path / "oc_cap"
    dev_route = "GET /api/v1/accounts/:id/account_developments"
    golden_file(other_canvas, route=dev_route, shape={"id": "integer"})
    capture_file(other_captured, {"nothing": 1}, route=dev_route)
    assert gd.main(["diff", "--canvas-dir", str(other_canvas),
                    "--capture-dir", str(other_captured),
                    "--deviations", str(deviations)]) == 1
