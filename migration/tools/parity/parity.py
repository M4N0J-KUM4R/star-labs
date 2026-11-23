#!/usr/bin/env python3
"""Canvas -> Rust parity scoreboard.

Two deterministic subcommands:

    manifest  extract Canvas API routes, source-size counts, and GraphQL
              operation-name inventories from a canvas-lms checkout into
              canvas_manifest.json
    report    compare the Rust workspace's axum routes and star-graphql
              type/mutation names against that manifest and write
              coverage.json, report.md, baseline.json

Stdlib only. No network, no timestamps, no absolute paths in the output.
"""

import argparse
import json
import re
import sys
from pathlib import Path

DRAW_RE = re.compile(r"^\s*ApiRouteSet(?:::V1)?\.draw\(self(?:,\s*\"([^\"]+)\")?\)\s+do\b")
VERB_RE = re.compile(r"^\s*(get|post|put|delete|patch|head|options|match)\b")
BLOCK_RE = re.compile(r"\bdo\b(?:\s*\|[^|]*\|)?\s*$")
COND_RE = re.compile(r"^\s*(if|unless|case|begin|while|until)\b")
END_RE = re.compile(r"^\s*end\b")
ASSIGN_RE = re.compile(r"^\s*([a-z_][a-z0-9_]*)\s*=\s*\"([^\"]*)\"\s*$")
COND_ASSIGN_RE = re.compile(
    r"^\s*([a-z_][a-z0-9_]*)\s*=\s*\(*\s*\(\s*([a-z_][a-z0-9_]*)"
    r"\s*==\s*\"([^\"]*)\"\s*\)\s*\?\s*([a-z_][a-z0-9_]*)\s*:\s*"
    r"\"([^\"]*)\"\s*\)*\s*$")
COND_EQ_RE = re.compile(
    r"^\s*(if|unless)\s+([a-z_][a-z0-9_]*)\s*==\s*\"([^\"]*)\"\s*$")
SCOPE_CTRL_RE = re.compile(r"controller:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_/]+)[\"'])")
ACTION_RE = re.compile(r"action:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_]+)[\"'])")
TO_RE = re.compile(r"to:\s*[\"']([A-Za-z0-9_/]+)#([A-Za-z0-9_]+)[\"']")
STR_RE = re.compile(r"[\"']([^\"']*)[\"']")
STAR_METHOD_RE = re.compile(r"\b(get|post|put|delete|patch|head|options)\s*\(")
# A method combinator in an axum chain, i.e. at the head of the `.route()`
# second argument or directly after a `.`: `axum::routing::get(h)` / `.post(h)`.
STAR_METHOD_HEAD_RE = re.compile(
    r"\s*(?:[A-Za-z_][A-Za-z0-9_]*::)*\s*(get|post|put|delete|patch|head|options)\s*\(")
EACH_RE = re.compile(r"^\s*(.+?)\.each\s+do\s*\|([^|]*)\|\s*$")
WORD_ARRAY_RE = re.compile(r"%w\[([^\]]*)\]")
RESOURCES_RE = re.compile(r"^\s*resources\s+:([a-z_][a-z0-9_]*)\b(.*)$")
# Rails' singular `resource :name` — same DSL but there is no index and the
# member path is the collection path (no `:id` segment).
SINGULAR_RESOURCE_RE = re.compile(r"^\s*resource\s+:([a-z_][a-z0-9_]*)\b(.*)$")
CONCERN_DEF_RE = re.compile(
    r"^\s*concern\s+:([a-z_][a-z0-9_]*)\b[^\n]*\bdo\b\s*(?:#.*)?$")
CONCERN_USE_RE = re.compile(
    r"^\s*concerns?\s+:([a-z_][a-z0-9_]*(?:\s*,\s*:[a-z_][a-z0-9_]*)*)"
    r"(?!\s+do\b)(.*)$")
MEMBER_BLOCK_RE = re.compile(r"^\s*(member|collection)\s+do\b")
PATH_OPT_RE = re.compile(
    r"\bpath:\s*(?:[\"']([^\"']*)[\"']|:([a-z_][a-z0-9_]*))")
ONLY_OPT_RE = re.compile(r"\bonly:\s*(%i\[([^\]]*)\]|\[([^\]]*)\]|:([A-Za-z0-9_]+))")
EXCEPT_OPT_RE = re.compile(r"\bexcept:\s*(%i\[([^\]]*)\]|\[([^\]]*)\]|:([A-Za-z0-9_]+))")
INTERP_RE = re.compile(r"#\{([^{}]*)\}")

# Rails' API resource defaults: no new/edit forms.
RESOURCE_ACTIONS = (
    ("index", "GET", False),
    ("create", "POST", False),
    ("show", "GET", True),
    ("update", "PATCH", True),
    ("update", "PUT", True),
    ("destroy", "DELETE", True),
)
# Singular `resource` has no index; everything else hangs off one path.
SINGULAR_RESOURCE_ACTIONS = (
    ("show", "GET", True),
    ("create", "POST", False),
    ("update", "PATCH", True),
    ("update", "PUT", True),
    ("destroy", "DELETE", True),
)
JS_ENV_RE = re.compile(r"\bjs_env\b")
UI_API_RE = re.compile(r"[\"'`](/api/v1/[^\"'`\s]*)")

# --- Canvas GraphQL name extraction -----------------------------------------
# Class declarations in app/graphql (both styles Canvas uses):
#   class Mutations::CreateAssignment < Mutations::AssignmentBase::Mutation
#   module Types \n class CourseType < ApplicationObjectType
RUBY_CLASS_RE = re.compile(
    r"^\s*class\s+((?:[A-Za-z0-9_]+::)*[A-Za-z0-9_]+)\s*<\s*([A-Za-z0-9_:]+)")
GRAPHQL_NAME_RE = re.compile(r"^\s*graphql_name\s+[\"']([A-Za-z0-9_]+)[\"']")
RUBY_END_RE = re.compile(r"^\s*end\b")

# Superclasses that make a class in types/*_type.rb a GraphQL schema type
# (verified inventory over canvas-lms master: ApplicationObjectType 153,
# BaseEnum 56, BaseInputObject 26, BaseUnion 6, BaseScalar 3, BaseEdge 1,
# GraphQL::Schema::Object 2, one nested AssignmentType subclass; Loader /
# FieldExtension / ForeignKeyLoader helpers are excluded).
GRAPHQL_TYPE_SUPERS = {
    "ApplicationObjectType", "BaseEnum", "BaseInputObject", "BaseUnion",
    "BaseScalar", "BaseEdge", "Object", "AssignmentType",
}

# Abstract mutation base classes: they subclass (or are) Mutations::BaseMutation
# but define no operation themselves.
MUTATION_ABSTRACT_BASES = {
    "BaseMutation", "BaseLearningOutcomeMutation", "BaseAiFeedback", "Mutation",
    "AllocationRuleBase", "DiscussionBase", "GroupSetBase",
    "OutcomeCalculationMethodBase", "OutcomeProficiencyBase",
}


# ---------------------------------------------------------------------------
# Path normalization
# ---------------------------------------------------------------------------

def normalize_path(path):
    """Strip /api/v1, collapse :params / {params} / interpolations to {}."""
    p = path.strip().split("?")[0]
    p = re.sub(r"^https?://[^/]+", "", p)
    p = re.sub(r"^/api/v1(?=/|$)", "", p)
    p = re.sub(r"[#$]\{[^}]*\}", ":__param__", p)
    p = re.sub(r"#.*$", "", p)  # URI fragment, after #{} interpolation is gone
    # A Rails glob segment (`*full_path`) is one path segment, like the
    # axum catch-all (`{*full_path}`) the port registers.
    p = re.sub(r"\*[A-Za-z_][A-Za-z0-9_]*", ":__param__", p)
    p = re.sub(r"\{[^/{}]*\}", ":__param__", p)
    p = re.sub(r":[A-Za-z_][A-Za-z0-9_]*", "{}", p)
    p = p.replace(":{}", "{}")  # :#{...} became ::__param__
    p = re.sub(r"\([^)]*\)", "", p)  # ruby glob fragments such as (/*scope)
    p = re.sub(r"/{2,}", "/", p)
    return p.strip("/")


def normalize_ui_reference(path):
    """UI literals often embed concrete ids; treat digit-only segments as params."""
    p = normalize_path(path)
    p = re.sub(r"(?<=/)\d+(?=/|$)", "{}", p)
    return p


# ---------------------------------------------------------------------------
# Canvas routes.rb parsing
# ---------------------------------------------------------------------------

def _draw_prefix(line):
    m = DRAW_RE.match(line)
    if not m:
        return None
    return m.group(1) or "/api/v1"


def _needs_continuation(text):
    if text.rstrip().endswith(","):
        return True
    depth = 0
    quote = None
    escaped = False
    for ch in text:
        if quote:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
        elif ch in "({[":
            depth += 1
        elif ch in ")}]":
            depth -= 1
    return depth > 0


def _join_statement(lines, index):
    """Join a Ruby route declaration that continues over several lines."""
    text = lines[index].rstrip("\n")
    while _needs_continuation(text) and index + 1 < len(lines):
        index += 1
        text += " " + lines[index].strip()
    return text, index


def _frame(indent, controller=None, variables=None, bindings=None,
           resource=None, base=None):
    return {
        "indent": indent,
        "controller": controller,
        "variables": variables if variables is not None else {},
        "bindings": bindings if bindings is not None else [{}],
        "resource": resource,
        "base": base,
    }


def _route_base(stack, prefix):
    """The path prefix Rails appends a relative route to.

    A `resources ... do` frame rebinds it to the collection path, and a
    `member`/`collection` frame inside that rebinds it again; every other
    block (scope, constraints, if) inherits whatever the enclosing frame set.
    """
    for frame in reversed(stack):
        if frame["base"]:
            return frame["base"]
    return prefix


def _pluralize(word):
    """The subset of ActiveSupport pluralization used by context loops."""
    if word.endswith(("s", "x", "z", "ch", "sh")):
        return word + "es"
    if word.endswith("y") and len(word) > 1 and word[-2] not in "aeiou":
        return word[:-1] + "ies"
    return word + "s"


def _singularize(word):
    if word.endswith("ies"):
        return word[:-3] + "y"
    if word.endswith(("ses", "xes", "zes", "ches", "shes")):
        return word[:-2]
    if word.endswith("s"):
        return word[:-1]
    return word


def _expand(text, bindings, variables, depth=0):
    """Substitute #{var}, #{var.pluralize}, and chained string variables."""
    if depth >= 8 or "#{" not in text:
        return text

    def replace(match):
        expr = match.group(1).strip()
        plural = False
        if expr.endswith(".pluralize"):
            expr = expr[:-len(".pluralize")]
            plural = True
        if not re.match(r"^[a-z_][a-z0-9_]*$", expr):
            return match.group(0)
        value = bindings.get(expr)
        if value is None:
            value = variables.get(expr)
        if isinstance(value, dict):
            if bindings.get(value["var"]) == value["equals"]:
                value = bindings.get(value["then"])
            else:
                value = _expand(value["else"], bindings, variables, depth + 1)
        if value is None:
            return match.group(0)
        return _pluralize(value) if plural else value

    return _expand(INTERP_RE.sub(replace, text), bindings, variables,
                   depth + 1)


def _parse_each_values(line):
    """Parse `%w[...].each do |x|` and `[%w[a b], %w[c d]].each do |(x, y)|`."""
    match = EACH_RE.match(line)
    if not match:
        return None
    receiver = match.group(1).strip()
    names = [name.strip() for name in
             match.group(2).replace("(", "").replace(")", "").split(",")]
    names = [name for name in names if name]
    if not names:
        return None
    if receiver.startswith("%w[") and receiver.endswith("]"):
        tokens = receiver[3:-1].split()
        if len(names) != 1 or not tokens:
            return None
        return [{names[0]: token} for token in tokens]
    if receiver.startswith("[") and receiver.endswith("]"):
        groups = WORD_ARRAY_RE.findall(receiver)
        if not groups:
            return None
        values = []
        for group in groups:
            tokens = group.split()
            if len(tokens) != len(names):
                return None
            values.append(dict(zip(names, tokens)))
        return values
    return None


def _combine_bindings(parent, values):
    return [dict(binding, **value) for binding in parent for value in values]


def _scope_controller(stack):
    for frame in reversed(stack):
        if frame["resource"] is None and frame["controller"]:
            return frame["controller"]
    return None


def _action_set(match):
    if not match:
        return None
    actions = match.group(2) or match.group(3) or match.group(4) or ""
    return {action.lstrip(":") for action in re.split(r"[\s,]+", actions)
            if action.strip()}


def _build_resource_routes(name, options, stack, prefix, singular=False):
    path_match = PATH_OPT_RE.search(options)
    controller_match = SCOPE_CTRL_RE.search(options)
    controller = (controller_match.group(1) or controller_match.group(2)) \
        if controller_match else None
    if controller is None:
        controller = _scope_controller(stack) or name
    only = _action_set(ONLY_OPT_RE.search(options))
    except_ = _action_set(EXCEPT_OPT_RE.search(options))
    segment = (path_match.group(1) or path_match.group(2)) \
        if path_match else name

    parent = None
    for frame in reversed(stack):
        if frame["resource"]:
            parent = frame["resource"]
            break
    base = _route_base(stack, prefix).rstrip("/")
    if parent:
        inherited = {parent["collection"].rstrip("/"): parent["nested"],
                     parent["nested"].rstrip("/"): parent["nested"],
                     parent["member"].rstrip("/"): parent["member"]}.get(base)
    else:
        inherited = None
    # `resources :y` inside `resources :x` nests under :x_id (Rails' parent
    # segment); inside `member do` it hangs off /:id; anywhere else it is
    # relative to whatever base the enclosing block established.
    collection = re.sub(
        r"/{2,}", "/",
        (inherited if inherited is not None else base)
        + "/" + segment.lstrip("/"))
    if singular:
        # Rails' `resource :x` serves one record: the member path is the
        # collection path and nested resources hang straight off it.
        member = collection
        nested = collection
    else:
        member = collection + "/:id"
        nested = collection + "/:" + _singularize(name) + "_id"
    resource = {
        "name": name,
        "collection": collection,
        "member": member,
        "nested": nested,
        "controller": controller,
    }

    routes = []
    seen = set()
    bindings = stack[-1]["bindings"]
    actions = SINGULAR_RESOURCE_ACTIONS if singular else RESOURCE_ACTIONS
    for action, verb, is_member in actions:
        if only is not None and action not in only:
            continue
        if except_ is not None and action in except_:
            continue
        for binding in bindings:
            path = _expand(member if is_member else collection, binding, {})
            original = path if path.startswith("/") else \
                prefix.rstrip("/") + "/" + path.lstrip("/")
            original = re.sub(r"/{2,}", "/", original)
            key = (verb, original, controller, action)
            if key in seen:
                continue
            seen.add(key)
            routes.append({
                "verb": verb,
                "path": original,
                "normalized_path": normalize_path(original),
                "controller": controller,
                "action": action,
            })
    return routes, resource


def _block_body(lines, start, start_indent):
    """Lines strictly between a Ruby block opener and its matching `end`."""
    body = []
    i = start
    while i < len(lines):
        line = lines[i]
        indent = len(line) - len(line.lstrip())
        if END_RE.match(line) and indent <= start_indent:
            return body, i + 1
        body.append(line)
        i += 1
    return body, i


def _extract_concerns(lines):
    """`concern :name do ... end` bodies, keyed by name.

    Rails shares route groups between API scopes this way; the same body is
    re-read in the enclosing scope of every `concerns :name` use, so a
    definition contributes nothing on its own.
    """
    concerns = {}
    i = 0
    while i < len(lines):
        m = CONCERN_DEF_RE.match(lines[i])
        if m:
            indent = len(lines[i]) - len(lines[i].lstrip())
            body, i = _block_body(lines, i + 1, indent)
            concerns[m.group(1)] = body
            continue
        i += 1
    return concerns


def _scan_lines(lines, i, end_index, stop_indent, prefix, stack, routes,
                concerns, skip_draw=False):
    """Route statements in `lines[i:end_index]` under `prefix`/`stack`.

    `stop_indent` ends a real Ruby block; concern bodies are slices with the
    closing `end` removed, so they pass None and run to `end_index`.
    `skip_draw` is the web-route pass: nested `ApiRouteSet.draw` blocks belong
    to the API manifest and must not be counted twice.
    """
    while i < end_index:
        line = lines[i]
        stripped = line.strip()
        indent = len(line) - len(line.lstrip())

        if skip_draw and DRAW_RE.match(line):
            _, i = _block_body(lines, i + 1, indent)
            continue

        if END_RE.match(line):
            if stop_indent is not None and indent <= stop_indent:
                break
            while len(stack) > 1 and stack[-1]["indent"] >= indent:
                stack.pop()
            i += 1
            continue

        if not stripped or stripped.startswith("#"):
            i += 1
            continue

        concern_use = CONCERN_USE_RE.match(line)
        if concern_use:
            names = [name.strip().lstrip(":") for name in
                     concern_use.group(1).split(",")]
            i += 1
            for name in names:
                body = concerns.get(name)
                if body is None:
                    continue
                _scan_lines(body, 0, len(body), None, prefix,
                            [dict(frame, bindings=list(frame["bindings"]),
                                  variables=dict(frame["variables"]))
                             for frame in stack],
                            routes, concerns, skip_draw)
            continue

        if VERB_RE.match(line):
            statement, i = _join_statement(lines, i)
            i += 1
            routes.extend(
                _parse_route_statement(statement, prefix, stack))
            continue

        loop_values = _parse_each_values(stripped)
        if loop_values is not None:
            stack.append(_frame(
                indent, None, dict(stack[-1]["variables"]),
                _combine_bindings(stack[-1]["bindings"], loop_values)))
            i += 1
            continue

        member_block = MEMBER_BLOCK_RE.match(line)
        if member_block and stack[-1]["resource"] is not None:
            resource = stack[-1]["resource"]
            kind = member_block.group(1)
            stack.append(_frame(
                indent, resource["controller"], dict(stack[-1]["variables"]),
                stack[-1]["bindings"], resource,
                base=resource["member"] if kind == "member"
                else resource["collection"]))
            i += 1
            continue

        if RESOURCES_RE.match(line) or SINGULAR_RESOURCE_RE.match(line):
            statement, i = _join_statement(lines, i)
            i += 1
            plural = RESOURCES_RE.match(statement)
            match = plural or SINGULAR_RESOURCE_RE.match(statement)
            resource_routes, resource = _build_resource_routes(
                match.group(1), match.group(2), stack, prefix,
                singular=plural is None)
            routes.extend(resource_routes)
            if BLOCK_RE.search(statement):
                # Rails nests a `resources :x do ... end` block's own route
                # strings under the parent segment (`/courses/:course_id/
                # grading_schemes` in Canvas' web roster tabs), so the frame
                # base is the nested path, not the collection.
                stack.append(_frame(
                    indent, resource["controller"],
                    dict(stack[-1]["variables"]), stack[-1]["bindings"],
                    resource, base=resource["nested"]))
            continue

        conditional = COND_ASSIGN_RE.match(line)
        if conditional:
            target, var, equals, then_var, else_template = \
                conditional.groups()
            stack[-1]["variables"][target] = {
                "var": var,
                "equals": equals,
                "then": then_var,
                "else": else_template,
            }
            i += 1
            continue

        assign = ASSIGN_RE.match(line)
        if assign:
            stack[-1]["variables"][assign.group(1)] = assign.group(2)
            i += 1
            continue

        condition = COND_EQ_RE.match(line)
        if condition:
            kind, var, equals = condition.groups()
            kept = [
                binding for binding in stack[-1]["bindings"]
                if (binding.get(var) == equals) == (kind == "if")
            ]
            stack.append(_frame(indent, None, dict(stack[-1]["variables"]),
                                kept))
            i += 1
            continue

        if BLOCK_RE.search(line) or COND_RE.match(line):
            ctrl = None
            m = SCOPE_CTRL_RE.search(line)
            if m:
                ctrl = m.group(1) or m.group(2)
            stack.append(_frame(indent, ctrl, dict(stack[-1]["variables"]),
                                stack[-1]["bindings"]))
        i += 1
    return i


def parse_canvas_routes(routes_file):
    lines = routes_file.read_text(encoding="utf-8", errors="replace").splitlines()
    routes = []
    concerns = _extract_concerns(lines)
    i = 0
    while i < len(lines):
        prefix = _draw_prefix(lines[i])
        if prefix is None:
            i += 1
            continue

        start_indent = len(lines[i]) - len(lines[i].lstrip())
        stack = [_frame(start_indent)]
        i += 1
        i = _scan_lines(lines, i, len(lines), start_indent, prefix, stack,
                        routes, concerns)
        i += 1

    return routes


# The application route set: everything in here that is not inside an
# ApiRouteSet block is a Canvas *web* route (HTML/JS-facing), which the UI
# shell and some star crates answer with JSON.
WEB_DRAW_RE = re.compile(
    r"^\s*[A-Za-z0-9_:]+\.routes\.draw\b[^\n]*\bdo\b\s*$")


def parse_canvas_web_routes(routes_file):
    """Canvas routes outside ApiRouteSet: the same file, the opposite filter."""
    lines = routes_file.read_text(encoding="utf-8", errors="replace").splitlines()
    concerns = _extract_concerns(lines)
    routes = []
    for index, line in enumerate(lines):
        if not WEB_DRAW_RE.match(line):
            continue
        indent = len(line) - len(line.lstrip())
        stack = [_frame(indent)]
        _scan_lines(lines, index + 1, len(lines), indent, "", stack, routes,
                    concerns, skip_draw=True)
        break
    seen = set()
    unique = []
    for route in routes:
        key = (route["verb"], route["path"])
        if key in seen:
            continue
        seen.add(key)
        unique.append(route)
    return unique


def _parse_route_statement(statement, prefix, stack):
    m = VERB_RE.match(statement)
    if not m:
        return []
    token = m.group(1)
    if token == "match":
        via = re.search(r"via:\s*\[([^\]]*)\]", statement)
        verbs = re.findall(r":?([A-Za-z]+)", via.group(1)) if via else []
        verbs = [v.upper() for v in verbs] or ["MATCH"]
    else:
        verbs = [token.upper()]

    rest = statement[m.end():].lstrip()
    base = _route_base(stack, prefix)
    symbol_action = None
    if rest[:1] in ("\"", "'"):
        sm = STR_RE.match(rest)
        raw_path = sm.group(1) if sm else None
    else:
        var = re.match(r"([A-Za-z_][A-Za-z0-9_]*)(?:\.to_s)?\s*[,)]", rest)
        raw_path = stack[-1]["variables"].get(var.group(1)) if var else None
        if raw_path is None:
            # Rails' shorthand: `get :questions` inside a `resources` block is
            # a MEMBER route at <collection>/:id/questions answered by the
            # block's controller. A quoted path never takes this branch.
            sym = re.match(r":([A-Za-z_][A-Za-z0-9_]*)\s*(?:[,)]|$)", rest)
            if sym and stack[-1]["resource"] is not None:
                symbol_action = sym.group(1)
                raw_path = stack[-1]["resource"]["member"] + "/" + symbol_action
            else:
                return []
    if not raw_path:
        return []

    controller = None
    cm = re.search(r"controller:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_/]+)[\"'])",
                   statement)
    if cm:
        controller = cm.group(1) or cm.group(2)
    if controller is None:
        for frame in reversed(stack):
            if frame["controller"]:
                controller = frame["controller"]
                break

    am = ACTION_RE.search(statement)
    action = (am.group(1) or am.group(2)) if am else None
    if action is None and symbol_action:
        action = symbol_action
    if action is None or controller is None:
        tm = TO_RE.search(statement)
        if tm:
            controller = controller or tm.group(1)
            action = action or tm.group(2)

    variables = stack[-1]["variables"]
    routes = []
    seen = set()
    for verb in verbs:
        for binding in stack[-1]["bindings"]:
            path = _expand(raw_path, binding, variables)
            if path.startswith("/"):
                original = path
            else:
                original = base.rstrip("/") + "/" + path.lstrip("/")
            original = re.sub(r"/{2,}", "/", original)
            for variant in _optional_variants(original):
                key = (verb, variant, controller, action)
                if key in seen:
                    continue
                seen.add(key)
                routes.append({
                    "verb": verb,
                    "path": variant,
                    "normalized_path": normalize_path(variant),
                    "controller": controller,
                    "action": action,
                })
    return routes


def _optional_variants(path, depth=0):
    """Rails registers `(/:id)` twice: with the segment present and absent.

    Without this, an optional tail (`…/tool_consumer_profile(/:id)`) is only
    ever measured as its short form, so the long form that star serves reads
    as a fabricated extra.
    """
    if depth > 4:
        return [path]
    m = re.search(r"\((/[^)]*)\)", path)
    if not m:
        return [path]
    without = path[:m.start()] + path[m.end():]
    with_seg = path[:m.start()] + m.group(1) + path[m.end():]
    out = []
    for variant in (_optional_variants(without, depth + 1)
                    + _optional_variants(with_seg, depth + 1)):
        if variant not in out:
            out.append(variant)
    return out


# ---------------------------------------------------------------------------
# Canvas source-size counts and UI references
# ---------------------------------------------------------------------------

def _walk_files(root, suffixes):
    if not root.is_dir():
        return []
    found = []
    for path in root.rglob("*"):
        if path.is_file() and path.suffix in suffixes:
            found.append(path)
    return sorted(found)


def count_canvas_sources(canvas_src):
    graphql = _walk_files(canvas_src / "app" / "graphql", {".rb"})
    mutations = _walk_files(canvas_src / "app" / "graphql" / "mutations", {".rb"})
    types = [p for p in graphql if p.name.endswith("_type.rb")]

    features_dir = canvas_src / "ui" / "features"
    feature_dirs = sorted(p for p in features_dir.iterdir() if p.is_dir()) \
        if features_dir.is_dir() else []

    ui_files = [
        p for p in _walk_files(canvas_src / "ui", {".js", ".jsx", ".ts", ".tsx"})
        if "__tests__" not in p.parts and not p.name.endswith(".d.ts")
    ]
    erb_views = _walk_files(canvas_src / "app" / "views", {".erb"})

    js_env_files = 0
    js_env_sites = 0
    for top in ("app", "ui", "lib", "config"):
        for path in _walk_files(canvas_src / top, {".rb", ".erb", ".js", ".jsx",
                                                  ".ts", ".tsx"}):
            text = path.read_text(encoding="utf-8", errors="replace")
            sites = len(JS_ENV_RE.findall(text))
            if sites:
                js_env_files += 1
                js_env_sites += sites

    return {
        "graphql_files": len(graphql),
        # FILE counts, not the parity denominators. One *_type.rb can declare
        # several types, so the denominators are the name inventories
        # (graphql_type_names / graphql_mutation_names), never these.
        "graphql_type_files": len(types),
        "graphql_mutation_files": len(mutations),
        "models": len(_walk_files(canvas_src / "app" / "models", {".rb"})),
        "ui_feature_dirs": len(feature_dirs),
        "js_tsx_files": len(ui_files),
        "erb_views": len(erb_views),
        "js_env_files": js_env_files,
        "js_env_call_sites": js_env_sites,
    }


def canvas_graphql_type_names(canvas_src):
    """Distinct GraphQL type names defined in app/graphql/types/*_type.rb.

    A file may define several types (course_type.rb also defines
    SubmissionOrderField / SubmissionOrderCriteria). The GraphQL name is the
    explicit `graphql_name "X"` when present, else the class name with a
    trailing `Type` suffix stripped (SectionType -> Section), which is
    graphql-ruby's default derivation. Validated against Canvas's committed
    schema.graphql: every extracted name appears there.
    """
    names = set()
    for path in sorted((canvas_src / "app" / "graphql" / "types").glob("*_type.rb")):
        classes = []
        open_classes = []
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            match = RUBY_CLASS_RE.match(line)
            if match and "; end" not in line:
                classes.append({
                    "class": match.group(1).split("::")[-1],
                    "super": match.group(2).split("::")[-1],
                    "name": None,
                })
                open_classes.append(
                    (len(line) - len(line.lstrip()), len(classes) - 1))
                continue
            named = GRAPHQL_NAME_RE.match(line)
            if named and open_classes:
                idx = open_classes[-1][1]
                if classes[idx]["name"] is None:
                    classes[idx]["name"] = named.group(1)
                continue
            if RUBY_END_RE.match(line):
                indent = len(line) - len(line.lstrip())
                open_classes = [f for f in open_classes if f[0] < indent]
        for entry in classes:
            if entry["super"] not in GRAPHQL_TYPE_SUPERS:
                continue
            if entry["class"] == "ApplicationObjectType":
                continue  # abstract base in application_object_type.rb
            if entry["name"]:
                names.add(entry["name"])
            elif entry["class"].endswith("Type") and len(entry["class"]) > 4:
                names.add(entry["class"][:-4])
            else:
                names.add(entry["class"])
    return sorted(names)


def _ruby_camel(pascal):
    """PascalCase Ruby class name -> camelCase GraphQL field name.

    graphql-ruby registers `Mutations::CreateAssignment` as the field
    `createAssignment` (verified 1:1 against mutation_type.rb's
    `field :create_assignment, mutation: Mutations::CreateAssignment`).
    """
    return pascal[:1].lower() + pascal[1:]


def canvas_graphql_mutation_names(canvas_src):
    """GraphQL mutation operation names from app/graphql/mutations/**/*.rb.

    An operation is a class whose superclass chain reaches
    Mutations::BaseMutation, excluding the abstract base classes themselves.
    This handles every declaration style Canvas uses:
      class Mutations::CreateAssignment < Mutations::AssignmentBase::Mutation
      class Mutations::CreateDiscussionTopic < Mutations::DiscussionBase
      module Mutations / class AcceptEnrollmentInvitation < BaseMutation
    The returned names are the camelCase mutation field names
    (CreateAssignment -> createAssignment), matching how async-graphql
    exposes `async fn create_assignment` as `createAssignment`; the mapping
    was verified 1:1 against the 100 `field :x, mutation: Mutations::X`
    registrations in app/graphql/types/mutation_type.rb.
    """
    supers = {}
    for path in sorted((canvas_src / "app" / "graphql" / "mutations").glob("**/*.rb")):
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            match = RUBY_CLASS_RE.match(line)
            if match:
                supers.setdefault(match.group(1).split("::")[-1],
                                  match.group(2).split("::")[-1])

    def reaches_base_mutation(cls, seen):
        if cls in seen or cls not in supers:
            return False
        seen.add(cls)
        sup = supers[cls]
        return sup == "BaseMutation" or reaches_base_mutation(sup, seen)

    return sorted(_ruby_camel(cls) for cls in supers
                  if reaches_base_mutation(cls, set())
                  and cls not in MUTATION_ABSTRACT_BASES)


def scan_ui_api_references(canvas_src):
    refs = set()
    literal_count = 0
    for path in _walk_files(canvas_src / "ui", {".js", ".jsx", ".ts", ".tsx"}):
        text = path.read_text(encoding="utf-8", errors="replace")
        for match in UI_API_RE.finditer(text):
            literal_count += 1
            ref = normalize_ui_reference(match.group(1))
            if ref:
                refs.add(ref)
    return refs, literal_count


def route_referenced(normalized_path, ui_refs):
    if normalized_path in ui_refs:
        return True
    tail = normalized_path + "/"
    return any(ref.startswith(tail) and
               all(seg == "{}" for seg in ref[len(tail):].split("/") if seg)
               for ref in ui_refs)


# ---------------------------------------------------------------------------
# Star (Rust) parsing
# ---------------------------------------------------------------------------

def _extract_route_calls(text):
    """Yield the argument text of every `.route(...)` call, paren-balanced."""
    pos = 0
    while True:
        start = text.find(".route(", pos)
        if start < 0:
            break
        i = start + len(".route(")
        depth = 1
        in_string = False
        escaped = False
        while i < len(text) and depth:
            ch = text[i]
            if in_string:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == '"':
                    in_string = False
            elif ch == '"':
                in_string = True
            elif ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
            i += 1
        yield text[start + len(".route("):i - 1]
        pos = i


def _skip_balanced(text, open_index, opener="(", closer=")"):
    """Index just past the `closer` that balances the `opener` at open_index."""
    depth = 0
    i = open_index
    in_string = False
    escaped = False
    while i < len(text):
        ch = text[i]
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
        elif ch == '"':
            in_string = True
        elif ch == opener:
            depth += 1
        elif ch == closer:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return len(text)


def _star_verbs(tail):
    """HTTP verbs of one `.route(path, <tail>)` argument list.

    axum chains method combinators, so only the chain head and the
    dot-prefixed continuations are verbs: `.delete(delete(h))` registers DELETE
    once, and the inner `delete(h)` is the handler's own name. The old
    findall-based version counted that pair twice.
    """
    verbs = []
    tail = tail.lstrip()
    if tail.startswith(","):
        tail = tail[1:]
    i = 0
    while i < len(tail):
        match = STAR_METHOD_HEAD_RE.match(tail, i)
        if not match:
            break
        verbs.append(match.group(1))
        i = _skip_balanced(tail, match.end() - 1)
        while i < len(tail) and tail[i] in " \t\r\n":
            i += 1
        if i < len(tail) and tail[i] == ".":
            i += 1
            continue
        break
    return verbs


CFG_TEST_RE = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
TEST_ATTR_RE = re.compile(r"#\[\s*(?:tokio::)?(?:test|sqlx::test)\b")
MOD_DECL_RE = re.compile(
    r"^\s*(?:#\[\s*path\s*=\s*\"([^\"]+)\"\s*\]\s*)?"
    r"(?:pub(?:\([^)]*\))?\s+)?(?:self\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;")


def _strip_tests(text):
    """Drop `#[cfg(test)]` items and `#[test]` function bodies.

    Test scaffolding registers nothing at runtime, and a test that builds a
    sub-router inflates the registration count (and shows up as a phantom
    duplicate of the product route it exercises).
    """
    for _ in range(500):
        match = CFG_TEST_RE.search(text)
        if not match:
            break
        brace = text.find("{", match.end())
        semicolon = text.find(";", match.end())
        if semicolon >= 0 and (brace < 0 or semicolon < brace):
            # `#[cfg(test)] mod tests;` — a whole file we must not read.
            text = text[:match.start()] + text[semicolon + 1:]
            continue
        if brace < 0:
            break
        text = text[:match.start()] + text[_skip_balanced(text, brace, "{", "}"):]
    for _ in range(500):
        match = TEST_ATTR_RE.search(text)
        if not match:
            break
        brace = text.find("{", match.end())
        if brace < 0:
            break
        text = text[:match.start()] + text[_skip_balanced(text, brace, "{", "}"):]
    return text


def star_reachable_files(star_src):
    """.rs files under `crates/*/src/` that a crate's `mod` tree pulls in.

    A file no `mod` declaration names is not compiled, so its routes are not
    registrations. The real case (issue #233) is a scaffold copy left beside
    the implemented module; skipping it keeps the extras list to what boots.
    """
    reachable = set()
    stack = []
    for crate_dir in sorted((star_src / "crates").iterdir()):
        src = crate_dir / "src"
        if not src.is_dir():
            continue
        for entry in ("lib.rs", "main.rs"):
            path = src / entry
            if path.is_file():
                stack.append((path, src, entry))
    seen = set()
    while stack:
        path, crate_src, name = stack.pop()
        key = path.resolve()
        if key in seen or not path.is_file():
            continue
        seen.add(key)
        reachable.add(key)
        text = _strip_tests(path.read_text(encoding="utf-8", errors="replace"))
        for line in text.splitlines():
            match = MOD_DECL_RE.match(line)
            if not match:
                continue
            explicit, ident = match.group(1), match.group(2)
            if explicit:
                stack.append((path.parent / explicit, crate_src, name))
                continue
            # `mod x;` in lib.rs/main.rs resolves to src/x.rs or src/x/mod.rs;
            # in a nested file it resolves under that file's own directory.
            if name in ("lib.rs", "main.rs"):
                bases = [crate_src]
            else:
                stem_dir = path.parent / path.stem
                bases = [stem_dir, path.parent] if stem_dir.is_dir() \
                    else [path.parent]
            for base in bases:
                for cand in (base / f"{ident}.rs", base / ident / "mod.rs"):
                    if cand.is_file():
                        stack.append((cand, crate_src, name))
    return reachable


def parse_star_routes(star_src):
    routes = []
    calls = 0
    unparsed = 0
    test_calls = 0
    unreachable_calls = 0
    reachable = star_reachable_files(star_src)
    for path in sorted((star_src / "crates").glob("*/src/*.rs")):
        crate = path.parent.parent.name
        raw = path.read_text(encoding="utf-8", errors="replace")
        raw_calls = list(_extract_route_calls(raw))
        text_calls = list(_extract_route_calls(_strip_tests(raw)))
        test_calls += max(len(raw_calls) - len(text_calls), 0)
        if path.resolve() not in reachable:
            # Uncompiled file: count its calls as excluded rather than
            # silently crediting its routes to the product.
            unreachable_calls += len(raw_calls)
            continue
        for args in text_calls:
            calls += 1
            sm = STR_RE.search(args)
            if not sm:
                unparsed += 1
                continue
            route_path = sm.group(1)
            methods = _star_verbs(args[sm.end():])
            if not methods:
                unparsed += 1
                continue
            for method in methods:
                routes.append({
                    "verb": method.upper(),
                    "path": route_path,
                    "normalized_path": normalize_path(route_path),
                    "crate": crate,
                    "source": str(path.relative_to(star_src)),
                })
    return (sorted(routes, key=lambda r: (r["normalized_path"], r["verb"],
                                          r["crate"])), calls, unparsed,
            test_calls, unreachable_calls)


def count_star_sources(star_src):
    impl = {
        "graphql_files": 0,
        "ui_feature_dirs": 0,
        "js_tsx_files": 0,
        "erb_views": 0,
        "js_env_files": 0,
    }
    for path in star_src.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix in {".graphql", ".graphqls"}:
            impl["graphql_files"] += 1
        elif path.suffix == ".erb":
            impl["erb_views"] += 1
        elif path.suffix in {".js", ".jsx", ".ts", ".tsx"}:
            impl["js_tsx_files"] += 1
        elif path.name == "Cargo.toml" and "async-graphql" in path.read_text(
                encoding="utf-8", errors="replace"):
            impl["graphql_files"] += 1
    features = star_src / "ui" / "features"
    if features.is_dir():
        impl["ui_feature_dirs"] = sum(1 for p in features.iterdir() if p.is_dir())
    if (star_src / "ui").is_dir():
        for path in _walk_files(star_src / "ui", {".rb", ".erb", ".js", ".jsx",
                                                  ".ts", ".tsx"}):
            if JS_ENV_RE.search(path.read_text(encoding="utf-8", errors="replace")):
                impl["js_env_files"] += 1

    tables = set()
    migrations = star_src / "crates" / "star-db" / "migrations"
    for path in sorted(migrations.glob("*.sql")) if migrations.is_dir() else []:
        text = path.read_text(encoding="utf-8", errors="replace")
        tables.update(m.group(1).lower() for m in re.finditer(
            r"CREATE TABLE(?:\s+IF NOT EXISTS)?\s+([A-Za-z_][A-Za-z0-9_]*)",
            text, re.IGNORECASE))
    impl["tables"] = len(tables)
    return impl


# --- Star (Rust) GraphQL name extraction -------------------------------------

# star-graphql deliberately renames four Canvas types for clarity; the Canvas
# graphql_name is what the scoreboard matches against (each rename is
# documented in the corresponding star-graphql module doc comment).
STAR_GRAPHQL_TYPE_ALIASES = {
    "CourseSection": "Section",        # app/graphql/types/section_type.rb
    "EnrollmentTerm": "Term",          # app/graphql/types/term_type.rb
    "DiscussionTopic": "Discussion",   # app/graphql/types/discussion_type.rb
    "QuizQuestion": "QuizItem",        # app/graphql/types/quiz_item_type.rb
}

# async-graphql derives that make a struct a GraphQL schema type.
STAR_GRAPHQL_DERIVES = {
    "SimpleObject", "Interface", "Union", "InputObject", "Enum", "MergedObject",
}

RUST_DERIVE_RE = re.compile(r"^\s*#\[\s*derive\(([^)]*)\)\s*\]")
RUST_OBJECT_ATTR_RE = re.compile(r"^\s*#\[\s*Object(?:\s*\([^)]*\))?\s*\]")
RUST_STRUCT_RE = re.compile(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?struct\s+([A-Za-z0-9_]+)")
RUST_ENUM_RE = re.compile(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?enum\s+([A-Za-z0-9_]+)")
RUST_IMPL_RE = re.compile(r"^\s*impl\s+(?:<[^>]*>\s*)?([A-Za-z0-9_:]+)")
RUST_FN_RE = re.compile(
    r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")


def _rust_camel(name):
    """snake_case Rust fn name -> camelCase GraphQL field name (async-graphql)."""
    parts = name.split("_")
    return parts[0] + "".join(part[:1].upper() + part[1:] for part in parts[1:])


def _next_declaration(lines, start, pattern, limit=4):
    """Match `pattern` on the first declaration line after `start`.

    Skips nothing else: an attribute sits directly above its item, so the
    struct/impl must appear within a few lines or the attribute belongs to
    something the scoreboard does not track. Returns the match's line index,
    or None.
    """
    for j in range(start + 1, min(start + 1 + limit, len(lines))):
        stripped = lines[j].lstrip()
        if stripped.startswith("#["):
            # Lint-control attributes (allow/deny/expect) legitimately sit
            # between a derive and its item; skip them, but any other
            # attribute means the derive belongs to something else.
            if stripped.startswith(("#[allow", "#[deny", "#[expect")):
                continue
            return None
        match = pattern.match(lines[j])
        if match:
            return j
    return None


def _impl_body(lines, impl_index):
    """Lines of the impl body opened on `lines[impl_index]` (brace counted)."""
    depth = lines[impl_index].count("{") - lines[impl_index].count("}")
    body = []
    j = impl_index + 1
    while j < len(lines) and depth > 0:
        depth += lines[j].count("{") - lines[j].count("}")
        if depth > 0 or "}" not in lines[j]:
            body.append(lines[j])
        j += 1
    return body


def parse_star_graphql(star_src):
    """Implemented GraphQL type and mutation names from star-graphql sources.

    Types are structs with an async-graphql derive (SimpleObject, ...) or the
    target of an `#[Object]` impl (Course, User, Query, ...). Mutations are
    the functions of `#[Object]` impls whose target is the Mutation root or a
    `*Mutations` container merged into it (the MergedObject pattern), with
    async-graphql's snake_case -> camelCase field renaming applied.
    """
    types = set()
    mutations = set()
    root = star_src / "crates" / "star-graphql" / "src"
    if not root.is_dir():
        return [], []
    for path in sorted(root.rglob("*.rs")):
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for i, line in enumerate(lines):
            derive = RUST_DERIVE_RE.match(line)
            if derive:
                # Normalize 'async_graphql::Enum' / 'async_graphql::Enum(copy)'
                # style tokens to their bare derive names.
                names = {d.strip().split("::")[-1].split("(")[0]
                         for d in derive.group(1).split(",")}
                if names & STAR_GRAPHQL_DERIVES:
                    # async-graphql derives apply to structs AND enums
                    # (Enum/InputObject both); try struct first, then enum.
                    j = _next_declaration(lines, i, RUST_STRUCT_RE)
                    if j is not None:
                        types.add(RUST_STRUCT_RE.match(lines[j]).group(1))
                    else:
                        j = _next_declaration(lines, i, RUST_ENUM_RE)
                        if j is not None:
                            types.add(RUST_ENUM_RE.match(lines[j]).group(1))
                continue
            if RUST_OBJECT_ATTR_RE.match(line):
                j = _next_declaration(lines, i, RUST_IMPL_RE)
                if j is None:
                    continue
                target = RUST_IMPL_RE.match(lines[j]).group(1).split("::")[-1]
                if target == "Mutation" or target.endswith("Mutations"):
                    for body_line in _impl_body(lines, j):
                        fn = RUST_FN_RE.match(body_line)
                        if fn:
                            mutations.add(_rust_camel(fn.group(1)))
                else:
                    types.add(target)
    return sorted(types), sorted(mutations)


# ---------------------------------------------------------------------------
# Percent helper
# ---------------------------------------------------------------------------

def pct(part, total):
    return round(100.0 * part / total, 1) if total else 0.0


def fmt_pct(part, total):
    return "%.1f%%" % pct(part, total)


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# manifest
# ---------------------------------------------------------------------------

def build_manifest(canvas_src):
    routes_file = canvas_src / "config" / "routes.rb"
    if not routes_file.is_file():
        raise SystemExit("not a Canvas checkout: %s" % routes_file)

    routes = parse_canvas_routes(routes_file)
    web = parse_canvas_web_routes(routes_file)
    ui_refs, ui_literals = scan_ui_api_references(canvas_src)
    for route in routes:
        route["ui_referenced"] = route_referenced(route["normalized_path"], ui_refs)

    counts = count_canvas_sources(canvas_src)
    counts["api_routes"] = len(routes)
    counts["web_routes"] = len(web)
    counts["controllers"] = len({r["controller"] for r in routes if r["controller"]})
    counts["ui_api_ref_strings"] = ui_literals
    counts["ui_api_ref_paths"] = len(ui_refs)

    return {
        "source": "canvas-lms config/routes.rb (ApiRouteSet blocks; resources "
                  "and %w context loops expanded; concerns and member/collection "
                  "blocks expanded)",
        "normalization": "strip /api/v1; :param, {param}, #{...} -> {}",
        "routes": routes,
        # Canvas' non-API route set. Star endpoints that match here are web/UI
        # ports, not REST parity items; see classify_extras.
        "web_routes": web,
        "counts": counts,
        "graphql_type_names": canvas_graphql_type_names(canvas_src),
        "graphql_mutation_names": canvas_graphql_mutation_names(canvas_src),
    }


def cmd_manifest(args):
    manifest = build_manifest(Path(args.canvas_src).resolve())
    write_json(Path(args.out), manifest)
    counts = manifest["counts"]
    print("canvas routes: %d (%d controllers)" %
          (counts["api_routes"], counts["controllers"]))
    print("ui-referenced: %d routes from %d /api/v1 literals" %
          (sum(1 for r in manifest["routes"] if r["ui_referenced"]),
           counts["ui_api_ref_strings"]))
    print("graphql names: %d types, %d mutation operations" %
          (len(manifest["graphql_type_names"]),
           len(manifest["graphql_mutation_names"])))
    print("wrote %s" % args.out)


# ---------------------------------------------------------------------------
# report
# ---------------------------------------------------------------------------

EXTRA_CLASSES = (
    "api_verb_mismatch",   # Canvas' API has this path and star misses that verb
    "api_extra_verb",      # Canvas' verbs here are all implemented; this is additive
    "api_alias",           # `self`/`me` literal for Canvas' `:user_id`-style param
    "web_exact",           # Canvas has this path+verb, but as a WEB route
    "web_verb_mismatch",   # Canvas has the path as a web route, other verb
    "web_nested",          # Canvas has it nested under a parent (web)
    "api_nested",          # manifest nests it under a parent star does not serve
    "api_extra_nested",    # manifest nests it, and that nested route IS implemented
    "absent",              # no Canvas path or path-tail match: alias, rename, or new
)


def _self_alias_paths(path):
    """Canvas' literal-`self` spelling of a star path, or () when there is none.

    `canvas-src/lib/api.rb:51` maps the id token `self` to
    `@current_user.id` for collections over the users table (the comment at
    `lib/api.rb:31` states the same), so `/api/v1/users/self/profile` and
    `/api/v1/users/:user_id/profile` are one Canvas route. star writes the
    alias out as its own registration, which a path matcher reports as absent —
    exactly the rows a "delete what Canvas does not have" pass would wrongly
    delete. Only `self` is proven in Canvas source; `me` is not, so it is not
    substituted here.
    """
    segments = path.split("/")
    return ["/".join("{}" if s == "self" else s for s in segments)] \
        if "self" in segments else []


def classify_extras(extras, canvas_routes, web_routes, star_keys):
    """Say WHY each unmatched star registration is unmatched.

    Before this existed every one of them was reported as a bare "extra", which
    conflated three different jobs: fix the verb, port a Canvas web endpoint,
    or delete a route Canvas never had.

    `api_*_mismatch`/`api_nested` now mean a real gap: Canvas answers this path
    in a shape star does not serve. When star already serves every Canvas verb
    at the path, the extra is additive (`api_extra_verb`) and the job is to
    label it, not to change it -- conflating the two put 18 no-op "verb fixes"
    on issue #233's list.
    """
    api_verbs = {}
    for route in canvas_routes:
        api_verbs.setdefault(route["normalized_path"], set()).add(route["verb"])
    web_verbs = {}
    for route in web_routes:
        web_verbs.setdefault(route["normalized_path"], set()).add(route["verb"])
    # endswith("/" + path) is the nesting test: Canvas nests most member
    # routes under a context, star registers them at the top level.
    api_paths = sorted(api_verbs, key=len, reverse=True)
    web_paths = sorted(web_verbs, key=len, reverse=True)
    api_routes_by_path = {}
    for route in canvas_routes:
        api_routes_by_path.setdefault(route["normalized_path"], []).append(route)

    web_keys = {(r["verb"], r["normalized_path"]) for r in web_routes}
    counts = {name: 0 for name in EXTRA_CLASSES}
    for extra in extras:
        path, verb = extra["normalized_path"], extra["verb"]
        if path in api_verbs:
            missing = sorted(v for v in api_verbs[path] if (v, path) not in star_keys)
            closest = sorted(api_verbs[path])
            kind = "api_verb_mismatch" if missing else "api_extra_verb"
            if missing:
                closest = missing
        elif any(ap in api_verbs for ap in _self_alias_paths(path)):
            ap = next(a for a in _self_alias_paths(path) if a in api_verbs)
            verbs = sorted(api_verbs[ap])
            # The alias only proves parity for the verb Canvas really answers
            # there: star's PUT /users/self/profile sits on a path Canvas
            # exposes as GET only, so it is an additive verb on an aliased
            # path, not a parity-exact route.
            kind = "api_alias" if verb in verbs else "api_extra_verb"
            counts[kind] += 1
            extra["classification"] = kind
            extra["canvas_closest"] = ["%s %s" % (v, ap) for v in verbs]
            continue
        elif (verb, path) in web_keys:
            kind = "web_exact"
            closest = ["WEB " + verb]
        elif path in web_verbs:
            kind = "web_verb_mismatch"
            closest = sorted(web_verbs[path])
        elif any(wp == path or wp.endswith("/" + path) for wp in web_paths):
            kind = "web_nested"
            closest = [wp for wp in web_paths
                       if wp.endswith("/" + path)][:1]
        elif any(ap == path or ap.endswith("/" + path) for ap in api_paths):
            parents = [ap for ap in api_paths if ap.endswith("/" + path)]
            served = [r for ap in parents for r in api_routes_by_path[ap]
                      if (r["verb"], r["normalized_path"]) in star_keys]
            missing = [r for ap in parents for r in api_routes_by_path[ap]
                       if (r["verb"], r["normalized_path"]) not in star_keys]
            kind = "api_extra_nested" if served and not missing else "api_nested"
            closest = [(r["verb"] + " " + r["normalized_path"])
                       for r in (missing or served)][:3]
        else:
            kind = "absent"
            closest = []
        extra["classification"] = kind
        extra["canvas_closest"] = closest
        counts[kind] += 1
    return counts


def build_coverage(manifest, star_src):
    canvas_routes = manifest["routes"]
    star_routes, calls, unparsed, test_calls, unreachable_calls = \
        parse_star_routes(star_src)
    star_keys = {(r["verb"], r["normalized_path"]) for r in star_routes}

    implemented = [r for r in canvas_routes if (r["verb"], r["normalized_path"]) in star_keys]
    ui_routes = [r for r in canvas_routes if r["ui_referenced"]]
    ui_implemented = [r for r in ui_routes if (r["verb"], r["normalized_path"]) in star_keys]

    canvas_keys = {(r["verb"], r["normalized_path"]) for r in canvas_routes}
    extras = [r for r in star_routes if (r["verb"], r["normalized_path"]) not in canvas_keys]
    extras_classes = classify_extras(extras, canvas_routes,
                                     manifest.get("web_routes") or [],
                                     star_keys)

    controllers = {}
    for route in canvas_routes:
        ctrl = route["controller"] or "(unknown)"
        entry = controllers.setdefault(ctrl, {"total": 0, "implemented": 0})
        entry["total"] += 1
        if (route["verb"], route["normalized_path"]) in star_keys:
            entry["implemented"] += 1
    controllers = {
        name: {
            "total": data["total"],
            "implemented": data["implemented"],
            "pct": pct(data["implemented"], data["total"]),
        }
        for name, data in controllers.items() if data["implemented"] > 0
    }

    star_impl = count_star_sources(star_src)
    counts = manifest["counts"]

    # GraphQL parity by operation name: implemented names from star-graphql
    # (with the documented renames mapped to Canvas graphql_names) intersected
    # with the Canvas inventories from the manifest.
    star_types, star_mutations = parse_star_graphql(star_src)
    canvas_types = set(manifest.get("graphql_type_names") or [])
    canvas_mutations = set(manifest.get("graphql_mutation_names") or [])
    type_alias = STAR_GRAPHQL_TYPE_ALIASES
    star_types_canonical = {type_alias.get(t, t) for t in star_types}
    impl_type_names = sorted(star_types_canonical & canvas_types)
    impl_mutation_names = sorted(set(star_mutations) & canvas_mutations)

    return {
        "summary": {
            "canvas_api_routes": len(canvas_routes),
            "canvas_web_routes": len(manifest.get("web_routes") or []),
            "canvas_controllers": counts["controllers"],
            "implemented_rest_routes": len(implemented),
            "implemented_rest_pct": pct(len(implemented), len(canvas_routes)),
            "ui_referenced_routes": len(ui_routes),
            "ui_referenced_implemented": len(ui_implemented),
            "ui_referenced_pct": pct(len(ui_implemented), len(ui_routes)),
            "star_route_registrations": len(star_routes),
            "star_route_calls": calls,
            "star_unparsed_calls": unparsed,
            "star_test_calls_excluded": test_calls,
            "star_unreachable_calls_excluded": unreachable_calls,
            "star_extras": len(extras),
            "star_extras_by_class": extras_classes,
            "graphql_files": counts["graphql_files"],
            "graphql_files_implemented": star_impl["graphql_files"],
            # Denominators are the NAME inventories the matcher actually
            # compares against, not count_canvas_sources' file-level scan
            # (170/110 there vs 246/100 names): mixing the two printed an
            # impossible 246/170.
            "graphql_types": len(canvas_types),
            "graphql_types_implemented": len(impl_type_names),
            "graphql_mutations": len(canvas_mutations),
            "graphql_mutations_implemented": len(impl_mutation_names),
            "ui_feature_dirs": counts["ui_feature_dirs"],
            "ui_feature_dirs_implemented": star_impl["ui_feature_dirs"],
            "js_tsx_files": counts["js_tsx_files"],
            "js_tsx_files_implemented": star_impl["js_tsx_files"],
            "erb_views": counts["erb_views"],
            "erb_views_implemented": star_impl["erb_views"],
            "js_env_files": counts["js_env_files"],
            "js_env_files_implemented": star_impl["js_env_files"],
            "js_env_call_sites": counts["js_env_call_sites"],
            "canvas_models": counts["models"],
            "star_tables": star_impl["tables"],
        },
        "controllers": controllers,
        "implemented_routes": implemented,
        "unmatched_star_routes": extras,
        "graphql": {
            "type_aliases": dict(sorted(type_alias.items())),
            "implemented_type_names": impl_type_names,
            "missing_type_names": sorted(canvas_types - star_types_canonical),
            "star_type_names_unmatched": sorted(
                star_types_canonical - canvas_types),
            "implemented_mutation_names": impl_mutation_names,
            "missing_mutation_names": sorted(
                canvas_mutations - set(star_mutations)),
            "star_mutation_names_unmatched": sorted(
                set(star_mutations) - canvas_mutations),
        },
    }


def render_report(coverage):
    s = coverage["summary"]
    lines = []
    add = lines.append
    add("# Canvas to Rust parity scoreboard")
    add("")
    add("Measured, deterministic output. Canvas source is not committed; refresh "
        "the manifest locally with `tools/parity/parity.py manifest`.")
    add("")
    add("## Summary")
    add("")
    add("| Area | Implemented | Total | Coverage |")
    add("|---|---:|---:|---:|")
    rows = [
        ("REST API routes", s["implemented_rest_routes"], s["canvas_api_routes"]),
        ("REST routes referenced from `ui/`", s["ui_referenced_implemented"],
         s["ui_referenced_routes"]),
        ("GraphQL files", s["graphql_files_implemented"], s["graphql_files"]),
        ("GraphQL object types", s["graphql_types_implemented"], s["graphql_types"]),
        ("GraphQL mutations", s["graphql_mutations_implemented"],
         s["graphql_mutations"]),
        ("UI feature directories", s["ui_feature_dirs_implemented"],
         s["ui_feature_dirs"]),
        ("JS/JSX/TS/TSX files", s["js_tsx_files_implemented"], s["js_tsx_files"]),
        ("ERB views", s["erb_views_implemented"], s["erb_views"]),
        ("js_env files", s["js_env_files_implemented"], s["js_env_files"]),
    ]
    for label, part, total in rows:
        add("| %s | %d | %d | %s |" % (label, part, total, fmt_pct(part, total)))
    add("| Schema tables (vs %d Canvas models) | %d | %d | n/a |" %
        (s["canvas_models"], s["star_tables"], s["canvas_models"]))
    add("")
    add("Implemented REST route count: **%d / %d** (%s)." %
        (s["implemented_rest_routes"], s["canvas_api_routes"],
         fmt_pct(s["implemented_rest_routes"], s["canvas_api_routes"])))
    add("")
    add("Star registrations: %d route/method pairs from %d `.route(...)` calls "
        "(%d unparsed); %d have no Canvas match and are listed as extras below." %
        (s["star_route_registrations"], s["star_route_calls"],
         s["star_unparsed_calls"], s["star_extras"]))
    add("")
    add("Excluded before counting: %d `.route(...)` calls inside `#[cfg(test)]` "
        "or `#[test]` bodies, %d calls in `.rs` files no `mod` declaration "
        "reaches (not compiled, so not registered)." %
        (s["star_test_calls_excluded"], s["star_unreachable_calls_excluded"]))
    add("")
    add("Canvas source sizes: %d GraphQL files (%d object types, %d mutations), "
        "%d models, %d UI feature dirs, %d JS/TSX files, %d ERB views, "
        "%d js_env files (%d call sites)." %
        (s["graphql_files"], s["graphql_types"], s["graphql_mutations"],
         s["canvas_models"], s["ui_feature_dirs"], s["js_tsx_files"],
         s["erb_views"], s["js_env_files"], s["js_env_call_sites"]))
    add("")

    ctrls = sorted(coverage["controllers"].items(),
                   key=lambda kv: (-kv[1]["implemented"], kv[0]))
    add("## Controllers with coverage (%d)" % len(ctrls))
    add("")
    add("| Controller | Implemented | Total | Coverage |")
    add("|---|---:|---:|---:|")
    for name, data in ctrls:
        add("| `%s` | %d | %d | %s |" %
            (name, data["implemented"], data["total"],
             fmt_pct(data["implemented"], data["total"])))
    add("")

    gql = coverage["graphql"]
    add("## GraphQL operation coverage")
    add("")
    add("Names are matched exactly: star-graphql type names (mapped through "
        "the documented renames below) against Canvas `graphql_name`s from "
        "`app/graphql/types/*_type.rb`, and mutation field names against the "
        "operations of `app/graphql/mutations/**` (classes whose superclass "
        "chain reaches `Mutations::BaseMutation`).")
    add("")
    add("### Implemented types (%d of %d Canvas names)" %
        (len(gql["implemented_type_names"]), s["graphql_types"]))
    add("")
    if gql["implemented_type_names"]:
        add(", ".join("`%s`" % n for n in gql["implemented_type_names"]))
    else:
        add("(none)")
    add("")
    if gql["type_aliases"]:
        add("Renames mapped to Canvas names: %s." % ", ".join(
            "`%s` -> `%s`" % (star, canvas)
            for star, canvas in gql["type_aliases"].items()))
        add("")
    add("### Missing types (%d)" % len(gql["missing_type_names"]))
    add("")
    if gql["missing_type_names"]:
        add(", ".join("`%s`" % n for n in gql["missing_type_names"]))
    else:
        add("(none)")
    add("")
    add("### Implemented mutations (%d of %d Canvas operations)" %
        (len(gql["implemented_mutation_names"]), s["graphql_mutations"]))
    add("")
    if gql["implemented_mutation_names"]:
        add(", ".join("`%s`" % n for n in gql["implemented_mutation_names"]))
    else:
        add("(none)")
    add("")
    add("### Missing mutations (%d)" % len(gql["missing_mutation_names"]))
    add("")
    if gql["missing_mutation_names"]:
        add(", ".join("`%s`" % n for n in gql["missing_mutation_names"]))
    else:
        add("(none)")
    add("")
    if gql["star_type_names_unmatched"] or gql["star_mutation_names_unmatched"]:
        add("Star names with no Canvas counterpart (not counted): %s." %
            ", ".join("`%s`" % n for n in gql["star_type_names_unmatched"] +
                      gql["star_mutation_names_unmatched"]))
        add("")

    extras = coverage["unmatched_star_routes"]
    add("## Star routes with no Canvas API match (%d)" % len(extras))
    add("")
    add("Classified, because the fix differs by class: only `api_verb_mismatch` "
        "and `api_nested` are Canvas gaps (star misses a verb or the nested form "
        "entirely); `api_extra_*` means Canvas' own verbs at that path are all "
        "implemented and this registration is *additive surface* -- Canvas does "
        "not answer that (verb, path) through its API, so each row still needs a "
        "routes.rb check before it is kept. It is not a clean bill of health: "
        "`POST /courses` sat here until it was deleted, because Canvas' only API "
        "course create is `post \"accounts/:account_id/courses\"` "
        "(canvas-src/config/routes.rb:1231); `web_*` means Canvas "
        "answers that path as a **web** (HTML/JS) route rather than an API "
        "route -- a port, not a REST parity item; `api_alias` is Canvas' own "
        "`self` id spelling (canvas-src/lib/api.rb:51 maps the literal `self` "
        "to the current user), which is parity-exact and must not be deleted; "
        "and `absent` means no Canvas path or path-tail matches, which covers "
        "deliberate aliases (`"
        "/assignment_overrides/{id}` for Canvas' "
        "`/courses/{course_id}/assignments/{assignment_id}/overrides/{id}`, "
        "whose nested form IS implemented) as well as genuinely new endpoints.")
    add("")
    classes = s.get("star_extras_by_class") or {}
    add("| Class | Count | Action |")
    add("|---|---:|---|")
    for name in EXTRA_CLASSES:
        action = {
            "api_verb_mismatch": "Canvas verb at this path is unimplemented: add it",
            "api_extra_verb": "Canvas serves other verbs here, not this one: "
                              "extra API surface, verify in routes.rb",
            "api_alias": "Canvas' `self` id alias (lib/api.rb): keep, parity-exact",
            "web_exact": "port of a Canvas web route: keep, label as web/UI",
            "web_verb_mismatch": "web port with the wrong verb",
            "web_nested": "web route flattened to top level: re-nest or label",
            "api_nested": "Canvas serves the nested form and star does not: re-nest",
            "api_extra_nested": "nested form already served: this flat twin is "
                                "extra surface, verify in routes.rb",
            "absent": "no Canvas path or tail: a renamed alias, a custom "
                      "endpoint, or unused -- check before deleting",
        }[name]
        add("| `%s` | %d | %s |" % (name, classes.get(name, 0), action))
    add("")
    add("| Verb | Path | Crate | Class | Canvas closest |")
    add("|---|---|---|---|---|")
    for route in sorted(extras, key=lambda r: (r.get("classification", "absent"),
                                               r["normalized_path"], r["verb"])):
        add("| %s | `%s` | `%s` | %s | %s |" %
            (route["verb"], route["path"], route["crate"],
             route.get("classification", "unclassified"),
             ", ".join("`%s`" % c for c in route.get("canvas_closest") or [])
             or "-"))
    add("")
    add("## Parser scope and limitations")
    add("")
    add("- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, "
        "plus Rails `resources` declarations (API defaults: index/create/show/"
        "update/destroy, honoring `only:`, `except:`, `path:`, `controller:`, "
        "and nesting) and `%w[...].each` context loops, which are expanded for "
        "every value.")
    add("- Conditional context prefixes assigned from "
        "`var == \"literal\" ? var : \"...\"` are evaluated per loop binding, "
        "and `if`/`unless var == \"literal\"` guards filter bindings; this "
        "expands the `outcome_groups_api` routes to `courses/...`, "
        "`accounts/...`, and `global/...`. Other runtime interpolations are "
        "left as `{}` and cannot match concrete Rust paths.")
    add("- UI references are `/api/v1/...` string literals in `ui/`; runtime-built "
        "paths and hard-coded ids that cannot be normalized are missed.")
    add("- GraphQL type names come from `graphql_name` declarations (or the "
        "class name minus a trailing `Type`) in `app/graphql/types/*_type.rb`, "
        "and mutation operations from classes whose superclass chain reaches "
        "`Mutations::BaseMutation` in `app/graphql/mutations/**`; the "
        "denominators remain the file counts (170 type files, 110 mutation "
        "files). star-graphql names are extracted from `#[Object]` impls and "
        "async-graphql derives, with documented renames "
        "(CourseSection->Section, EnrollmentTerm->Term, "
        "DiscussionTopic->Discussion, QuizQuestion->QuizItem) mapped to their "
        "Canvas graphql_names before matching. Connection and edge helpers "
        "(e.g. `PageInfo`, `*Edge`, `*Connection`) only count when Canvas "
        "defines a type of the same name.")
    add("- Rust paths are matched by `(verb, normalized path)`; axum's "
        "`nest(\"/api/v1\", ...)` mount is assumed for all crate routers.")
    add("- Extras include deliberate flat-path deviations (for example "
        "`/assignments/{id}` where Canvas uses "
        "`/courses/{course_id}/assignments/{id}`) and auth/dashboard/health "
        "routes Canvas does not expose.")
    add("- The schema row counts `CREATE TABLE` statements in `star-db` migrations "
        "(36 authored tables); sqlx adds `_sqlx_migrations`, so a live database "
        "has 37 tables.")
    add("")
    return "\n".join(lines)


def cmd_report(args):
    manifest = read_json(Path(args.manifest))
    star_src = Path(args.star_src).resolve()
    out_dir = Path(args.out)

    coverage = build_coverage(manifest, star_src)
    current = coverage["summary"]["implemented_rest_routes"]

    baseline_path = out_dir / "baseline.json"
    previous = read_json(baseline_path)["implemented_rest_routes"] \
        if baseline_path.is_file() else None

    write_json(out_dir / "coverage.json", coverage)
    (out_dir / "report.md").write_text(render_report(coverage), encoding="utf-8")

    regressed = previous is not None and current < previous
    s = coverage["summary"]
    # A parser that recognizes nothing reports 0 implemented routes, which is
    # not a coverage regression the baseline can catch when the baseline itself
    # is missing (first run, or a scratch --out). Fail loudly instead.
    parser_broken = (s["star_route_calls"] > 0
                     and s["star_unparsed_calls"] == s["star_route_calls"])
    if not regressed and not parser_broken:
        write_json(baseline_path, {
            "implemented_rest_routes": current,
            "policy": "report fails when implemented REST routes fall below this number",
        })

    s = coverage["summary"]
    print("REST: %d/%d (%s)" % (current, s["canvas_api_routes"],
                                fmt_pct(current, s["canvas_api_routes"])))
    print("GraphQL: %d/%d types, %d/%d mutations" %
          (s["graphql_types_implemented"], s["graphql_types"],
           s["graphql_mutations_implemented"], s["graphql_mutations"]))
    print("UI-referenced: %d/%d (%s)" %
          (s["ui_referenced_implemented"], s["ui_referenced_routes"],
           fmt_pct(s["ui_referenced_implemented"], s["ui_referenced_routes"])))
    print("star registrations: %d (%d extras)" %
          (s["star_route_registrations"], s["star_extras"]))
    print("excluded: %d test-code calls, %d calls in uncompiled files" %
          (s["star_test_calls_excluded"], s["star_unreachable_calls_excluded"]))
    print("wrote %s" % out_dir)

    if parser_broken:
        print("parity parse failure: all %d `.route(...)` calls unparsed — "
              "the star parser matched nothing, so every number above is "
              "meaningless" % s["star_route_calls"], file=sys.stderr)
        return 1
    if regressed:
        print("parity regression: implemented %d < baseline %d" %
              (current, previous), file=sys.stderr)
        return 1
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("manifest", help="extract Canvas routes and counts")
    p.add_argument("--canvas-src", required=True, help="Canvas LMS checkout")
    p.add_argument("--out", required=True, help="output JSON file")
    p.set_defaults(func=cmd_manifest)

    p = sub.add_parser("report", help="compare Rust routes to the manifest")
    p.add_argument("--manifest", required=True, help="canvas_manifest.json")
    p.add_argument("--star-src", required=True, help="migration workspace root")
    p.add_argument("--out", required=True, help="output directory")
    p.set_defaults(func=cmd_report)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
