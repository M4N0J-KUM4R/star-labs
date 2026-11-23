#!/usr/bin/env python3
"""Canvas-truth vs Star-Labs response-shape differ (Phase 2, issue #231).

The REST scoreboard measures *registration*: a `(verb, path)` pair exists in the
Rust router. It says nothing about whether the JSON body has the fields Canvas
returns. This tool compares the shape of a captured Star response against a
golden shape extracted from Canvas source, so missing and mistyped fields become
per-route findings instead of an unmeasured dimension.

Inputs
------
``canvas/<name>.json``   truth shapes, one per route. Every one is derived from
                         Canvas Ruby source and carries a ``source`` citation;
                         ``selfcheck`` refuses a golden whose citation is empty,
                         because an uncited shape is an opinion, not truth.
``captured/<name>.json``  our responses, written by the behavioral harness when
                         ``CAPTURE_GOLDEN_DIR`` is set:
                         ``{"route": "GET /api/v1/course_accounts", "body": ...}``
``deviations.json``      accepted differences, each with the Canvas source line
                         that makes the difference intentional. A deviation's
                         ``field`` addresses a *shape* position, and array
                         indices normalize to ``[*]``, so an acceptance written
                         for one captured record still covers the same field in
                         the second, third and hundredth.

Naming
------
``golden_filename`` mirrors the harness's own path handling
(``crates/star-http/examples/behavior_parity.rs``, ``canvas_base_path``) so a
Canvas manifest path and the Star route that serves it always produce the same
file name: ``GET /api/v1/course_accounts`` -> ``GET__api_v1_course_accounts.json``,
``GET /api/v1/courses/:id`` -> ``GET__api_v1_courses_param.json``.

Shape grammar
-------------
A shape is one of:

* a type token -- ``"string"``, ``"integer"``, ``"number"``, ``"boolean"``,
  ``"null"``, ``"any"``
* ``{"$optional": <shape>}`` -- under an object key, the key may be absent
* ``{"$nullable": <shape>}``  -- the value may be JSON ``null``
* ``{"$list": <shape>}``      -- an array whose elements all match ``<shape>``
* a JSON object -- every key is required unless wrapped in ``$optional``

Findings
--------
``FieldMiss``         a required Canvas key is absent from our body. Actionable.
``TypeMismatch``      present, but not the JSON type Canvas puts there. Actionable.
``UnexpectedNull``    ``null`` where the Canvas serializer cannot produce one.
                      Actionable.
``ExtraField``        a key we return that Canvas does not. Reported, NOT
                      actionable by default -- additive fields are how this
                      migration lands, and the scoreboard already classifies
                      extra routes the same way. ``--strict-extras`` promotes it.
``NoElements``        a ``$list`` captured empty. Reported, actionable only with
                      ``--require-capture``, because an anonymous capture of a
                      caller-scoped route is legitimately ``[]`` and proves
                      nothing about element shape.
``MissingCapture``    no capture file for a Canvas golden. Skipped, actionable
                      only with ``--require-capture``.

Exit status
-----------
``0``  no actionable findings.
``1``  one or more actionable findings.
``2``  usage or I/O error (bad golden, unreadable dir) -- never a parity verdict.

Nullability is read from the serializer, never from the column: Canvas marks an
attribute ``NOT NULL`` in the schema and still emits ``null`` for it, because
``account_json`` overwrites the key afterwards
(canvas-src/lib/api/v1/account.rb:41-42).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

TYPE_TOKENS = {"string", "integer", "number", "boolean", "null", "any"}
WRAPPERS = {"$optional", "$nullable", "$list"}

# Findings that fail the gate on their own. NoElements and MissingCapture are
# evidence gaps rather than shape disagreements, so they only gate when the
# caller asks for a corpus-wide verdict with --require-capture.
ACTIONABLE_CLASSES = {"FieldMiss", "TypeMismatch", "UnexpectedNull"}
OPTIONAL_WHEN_STRICT = "ExtraField"

_INDEX = re.compile(r"\[\d+\]")


def normalize_shape_path(path: str) -> str:
    """Collapse concrete array indices so an acceptance is position-independent."""
    return _INDEX.sub("[*]", path)


@dataclass
class Finding:
    route: str
    kind: str
    path: str
    detail: str
    accepted: bool = False

    def key(self) -> tuple[str, str, str]:
        return (self.route, normalize_shape_path(self.path), self.kind)

    def __str__(self) -> str:
        tag = "accepted" if self.accepted else self.kind
        return f"{self.route}  {self.path}: {self.detail} [{tag}]"


@dataclass
class Corpus:
    canvas: dict[str, dict] = field(default_factory=dict)
    captured: dict[str, dict] = field(default_factory=dict)
    deviations: set[tuple[str, str, str]] = field(default_factory=set)
    errors: list[str] = field(default_factory=list)


def api_path(verb: str, path: str) -> str:
    """The path the router actually serves, prefixed like the harness prefixes.

    Mirrors ``canvas_base_path`` in the behavioral harness: strip Canvas'
    optional-segment suffix (``(/:scope)``), and add ``/api/v1`` only when the
    route set is not already root-mounted.
    """
    base = path.split("(")[0].rstrip("/")
    if (
        base.startswith("/api/")
        or base.startswith("/auth/")
        or base in ("/login", "/logout", "/canvadoc_session")
    ):
        return base
    return "/api/v1" + base


def golden_filename(verb: str, path: str) -> str:
    """Stable file name for a route, identical for the Canvas and Star spelling."""
    segs = []
    for seg in api_path(verb, path).strip("/").split("/"):
        if seg.startswith(":") or seg.startswith("{") or seg.startswith("*"):
            segs.append("param")
        else:
            segs.append(seg)
    return f"{verb.upper()}__" + "_".join(segs) + ".json"


def _is_token(node: Any) -> bool:
    return isinstance(node, str) and node in TYPE_TOKENS


def _wrapper_of(node: Any) -> tuple[str, Any] | None:
    """A shape object with exactly one ``$`` key is a wrapper, not a record."""
    if isinstance(node, dict) and len(node) == 1:
        (key, val), = node.items()
        if key in WRAPPERS:
            return key, val
    return None


def _matches(token: str, value: Any) -> bool:
    # bool is an int subclass in Python; Canvas never sends a boolean where an
    # id is expected, so it must not satisfy "integer".
    if token == "string":
        return isinstance(value, str)
    if token == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if token == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if token == "boolean":
        return isinstance(value, bool)
    if token == "null":
        return value is None
    return True


def _json_type(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    return "object"


def diff_shape(route: str, node: Any, value: Any, at: str, out: list[Finding]) -> None:
    """Recursively compare one shape node against one captured value."""
    wrapper = _wrapper_of(node)
    if wrapper:
        kind, inner = wrapper
        if kind == "$nullable":
            if value is None:
                return
            diff_shape(route, inner, value, at, out)
            return
        if kind == "$list":
            if not isinstance(value, list):
                out.append(Finding(route, "TypeMismatch", at,
                                   f"expected array, got {_json_type(value)}"))
                return
            if not value:
                out.append(Finding(route, "NoElements", at,
                                   "array captured empty; element shape unverified"))
                return
            for i, element in enumerate(value):
                diff_shape(route, inner, element, f"{at}[{i}]", out)
            return
        # $optional at a non-key position: treat as "absent is fine".
        if value is None:
            return
        diff_shape(route, inner, value, at, out)
        return

    if _is_token(node):
        if value is None:
            out.append(Finding(route, "UnexpectedNull", at,
                               f"null where Canvas always emits a {node}"))
            return
        if not _matches(node, value):
            out.append(Finding(route, "TypeMismatch", at,
                               f"expected {node}, got {_json_type(value)}"))
        return

    if isinstance(node, dict):
        if not isinstance(value, dict):
            out.append(Finding(route, "TypeMismatch", at,
                               f"expected object, got {_json_type(value)}"))
            return
        for key, child in node.items():
            opt = _wrapper_of(child)
            optional = bool(opt and opt[0] == "$optional")
            if key not in value:
                if not optional:
                    out.append(Finding(route, "FieldMiss", f"{at}.{key}" if at else key,
                                       "Canvas returns this field; our body does not"))
                continue
            inner = opt[1] if optional else child
            diff_shape(route, inner, value[key], f"{at}.{key}" if at else key, out)
        for key in value:
            if key not in node:
                out.append(Finding(route, "ExtraField",
                                   f"{at}.{key}" if at else key,
                                   "we return a field Canvas does not"))
        return

    raise ValueError(f"malformed shape node at {at or '$'}: {node!r}")


def validate_golden(name: str, doc: Any) -> str | None:
    """Return an error string when a golden is not a usable shape document."""
    if not isinstance(doc, dict):
        return f"{name}: golden must be a JSON object"
    for req in ("route", "source", "shape"):
        if req not in doc:
            return f"{name}: golden is missing required key '{req}'"
    if not isinstance(doc["source"], str) or not doc["source"].strip():
        return f"{name}: 'source' must cite canvas-src, not be empty"
    try:
        _walk_validate(doc["shape"], "$")
    except ValueError as exc:
        return f"{name}: {exc}"
    return None


def _walk_validate(node: Any, at: str) -> None:
    wrapper = _wrapper_of(node)
    if wrapper:
        _walk_validate(wrapper[1], at)
        return
    if _is_token(node):
        return
    if isinstance(node, dict):
        for key, child in node.items():
            _walk_validate(child, f"{at}.{key}")
        return
    raise ValueError(f"malformed shape node at {at}: {node!r}")


def load_corpus(canvas_dir: Path, capture_dir: Path | None,
                deviations_path: Path | None) -> Corpus:
    corpus = Corpus()
    if not canvas_dir.is_dir():
        corpus.errors.append(f"canvas golden dir not found: {canvas_dir}")
        return corpus

    for path in sorted(canvas_dir.glob("*.json")):
        try:
            doc = json.loads(path.read_text())
        except json.JSONDecodeError as exc:
            corpus.errors.append(f"{path.name}: invalid JSON ({exc})")
            continue
        error = validate_golden(path.name, doc)
        if error:
            corpus.errors.append(error)
            continue
        corpus.canvas[path.name] = doc

    if capture_dir and capture_dir.is_dir():
        for path in sorted(capture_dir.glob("*.json")):
            try:
                doc = json.loads(path.read_text())
            except json.JSONDecodeError as exc:
                corpus.errors.append(f"{path.name}: invalid capture ({exc})")
                continue
            corpus.captured[path.name] = doc

    if deviations_path and deviations_path.is_file():
        try:
            doc = json.loads(deviations_path.read_text())
        except json.JSONDecodeError as exc:
            corpus.errors.append(f"{deviations_path.name}: invalid JSON ({exc})")
            return corpus
        for entry in doc.get("deviations", []):
            for req in ("route", "field", "class", "reason"):
                if req not in entry:
                    corpus.errors.append(
                        f"deviation entry missing '{req}': {entry}")
                    break
            else:
                corpus.deviations.add(
                    (entry["route"], normalize_shape_path(entry["field"]),
                     entry["class"]))
    return corpus


def compare(corpus: Corpus) -> list[Finding]:
    """Every finding for the corpus, gating and acceptance left to the caller.

    Whether a kind gates is deliberately not decided here: one call gives the
    full picture for the report, and ``is_actionable`` then applies the gate
    flags, so the printed findings do not depend on how strictly we chose to
    fail.
    """
    findings: list[Finding] = []
    for name, doc in sorted(corpus.canvas.items()):
        route = doc["route"]
        capture = corpus.captured.get(name)
        if capture is None:
            findings.append(Finding(route, "MissingCapture", "$",
                                    "no captured response for this golden"))
            continue
        findings.extend(_compare_route(route, doc, capture))

    for f in findings:
        f.accepted = f.key() in corpus.deviations
    return findings


def _compare_route(route: str, doc: dict, capture: dict) -> list[Finding]:
    out: list[Finding] = []
    if "body" not in capture:
        return [Finding(route, "MissingCapture", "$", "capture has no 'body' key")]
    diff_shape(route, doc["shape"], capture["body"], "", out)
    return out


def is_actionable(finding: Finding, require_capture: bool = False,
                  strict_extras: bool = False) -> bool:
    if finding.accepted:
        return False
    actionable = set(ACTIONABLE_CLASSES)
    if strict_extras:
        actionable.add(OPTIONAL_WHEN_STRICT)
    if require_capture:
        actionable.update({"NoElements", "MissingCapture"})
    return finding.kind in actionable


def summarize(findings: list[Finding]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for f in findings:
        counts[f.kind] = counts.get(f.kind, 0) + 1
    return counts


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("selfcheck", help="validate the Canvas golden corpus only")
    p.add_argument("--canvas-dir", required=True)
    p.add_argument("--deviations", default=None)

    p = sub.add_parser("diff", help="diff captured bodies against Canvas goldens")
    p.add_argument("--canvas-dir", required=True)
    p.add_argument("--capture-dir", required=True)
    p.add_argument("--deviations", default=None)
    p.add_argument("--require-capture", action="store_true",
                   help="gate on missing/empty captures too (use once every route is captured)")
    p.add_argument("--strict-extras", action="store_true",
                   help="promote ExtraField to a gate failure")
    p.add_argument("--json", action="store_true", help="emit findings as JSON")

    args = ap.parse_args(argv)

    canvas_dir = Path(args.canvas_dir)
    deviations = Path(args.deviations) if getattr(args, "deviations", None) else None
    capture_dir = Path(args.capture_dir) if args.cmd == "diff" else None

    corpus = load_corpus(canvas_dir, capture_dir, deviations)

    if corpus.errors:
        for err in corpus.errors:
            print(f"::error::golden corpus invalid: {err}", file=sys.stderr)
        return 2

    if args.cmd == "selfcheck":
        print(f"canvas goldens: {len(corpus.canvas)} validated")
        return 0

    findings = compare(corpus)
    bad = [f for f in findings
           if is_actionable(f, args.require_capture, args.strict_extras)]

    if args.json:
        print(json.dumps([vars(f) for f in findings], indent=1))
    else:
        for f in findings:
            print(str(f))
        counts = summarize(findings)
        print(f"\ncompared {len(corpus.captured)} capture(s) against "
              f"{len(corpus.canvas)} golden(s)")
        print("findings: " + (", ".join(f"{k}={v}" for k, v in sorted(counts.items()))
                              or "none"))
        print(f"actionable: {len(bad)}  accepted deviations: "
              f"{sum(1 for f in findings if f.accepted)}")

    for f in bad:
        print(f"::error::{f.route} {f.path}: {f.kind} -- {f.detail}", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
