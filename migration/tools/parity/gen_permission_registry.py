#!/usr/bin/env python3
"""Regenerate `crates/star-permissions/src/registry.rs` from Canvas.

The permission registry is Canvas' own contract: every permission, the roles
that hold it, and the rules that decide who may override it. The Rust port
carries it verbatim so that a route can ask the same question Canvas asks.

The data comes from two places:

* ``migration/parity/canvas-api-surface.json`` — the dump the running Canvas
  wrote through ``canvas-boot/canvas_introspection.rb``. It carries
  ``true_for`` / ``false_for`` / ``account_only`` / ``deprecated`` /
  ``not_for_masquerading`` for every permission.
* ``canvas-src/config/initializers/permissions_registry.rb`` — the Ruby
  source. It carries ``available_to`` / ``applies_to_concluded`` /
  ``account_allows`` / ``restrict_future_enrollments``, which the dump does
  not.

Run it from ``migration/``::

    python3 tools/parity/gen_permission_registry.py

It writes ``crates/star-permissions/src/registry.rs``.
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MIGRATION = os.path.dirname(os.path.dirname(HERE))
REPO = os.path.dirname(MIGRATION)


def find_canvas_sources():
    """Locate `canvas-src/`, which sits beside the Rust repo."""
    candidates = [
        os.path.join(REPO, "canvas-src"),
        os.path.join(os.path.dirname(REPO), "canvas-src"),
        os.path.join(MIGRATION, "canvas-src"),
    ]
    for candidate in candidates:
        registry = os.path.join(
            candidate, "config", "initializers", "permissions_registry.rb"
        )
        if os.path.exists(registry):
            return candidate, registry
    return None, None


CANVAS_SRC, REGISTRY_RB = find_canvas_sources()
SURFACE_JSON = os.path.join(MIGRATION, "parity", "canvas-api-surface.json")
OUT_RS = os.path.join(MIGRATION, "crates", "star-permissions", "src", "registry.rs")

ROLE_ORDER = [
    "StudentEnrollment",
    "TeacherEnrollment",
    "TaEnrollment",
    "DesignerEnrollment",
    "ObserverEnrollment",
    "AccountAdmin",
    "AccountMembership",
]


def split_blocks(path):
    """Split the Ruby registry into one block per permission."""
    blocks = {}
    name = None
    buf = []
    for line in open(path).read().split("\n"):
        header = re.match(r"^  ([a-z0-9_]+): \{$", line)
        if header:
            if name:
                blocks[name] = "\n".join(buf)
            name = header.group(1)
            buf = []
            continue
        if name is not None:
            if re.match(r"^  \},?$", line):
                blocks[name] = "\n".join(buf)
                name = None
                buf = []
                continue
            buf.append(line)
    if name:
        blocks[name] = "\n".join(buf)
    return blocks


def word_list(block, key):
    """Read a `%w[...]` or `[...]` list out of a Ruby block."""
    match = re.search(r"^\s*%s: %%w\[([^\]]*)\]" % key, block, re.M)
    if match:
        return match.group(1).split()
    match = re.search(r"^\s*%s: \[([^\]]*)\]" % key, block, re.M)
    if match:
        return [x.strip().strip('"') for x in match.group(1).split(",") if x.strip()]
    return None


def rust_roles(names):
    ordered = sorted({r for r in names if r in ROLE_ORDER}, key=ROLE_ORDER.index)
    if not ordered:
        return "&[]"
    return "&[" + ", ".join("RoleKind::" + r for r in ordered) + "]"


def rust_str(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    if not os.path.exists(REGISTRY_RB) or not os.path.exists(SURFACE_JSON):
        print("missing the Canvas sources; run this from a full checkout", file=sys.stderr)
        return 1

    blocks = split_blocks(REGISTRY_RB)
    surface = json.load(open(SURFACE_JSON))
    permissions = [p for p in surface["permissions"] if "name" in p]

    out = []
    for permission in sorted(permissions, key=lambda p: p["name"]):
        name = permission["name"]
        block = blocks.get(name)
        if block is None:
            print("no Ruby block for %s" % name, file=sys.stderr)
            return 1

        concluded = None
        if re.search(r"^\s*applies_to_concluded: true", block, re.M):
            concluded = "true"
        else:
            listed = word_list(block, "applies_to_concluded")
            if listed is not None:
                concluded = " ".join(listed)

        account_only = {
            "": "AccountOnly::None",
            "true": "AccountOnly::Account",
            "root": "AccountOnly::Root",
            "site_admin": "AccountOnly::SiteAdmin",
        }[permission["account_only"]]

        if concluded is None:
            concluded_expr = "Concluded::Never"
        elif concluded == "true":
            concluded_expr = "Concluded::Any"
        else:
            parts = sorted({r for r in concluded.split() if r in ROLE_ORDER}, key=ROLE_ORDER.index)
            concluded_expr = "Concluded::Roles(&[" + ", ".join("RoleKind::" + r for r in parts) + "])"

        out.append(
            "    PermissionSpec {{\n"
            "        name: {name},\n"
            "        label: {label},\n"
            "        true_for: {true_for},\n"
            "        false_for: {false_for},\n"
            "        available_to: {available_to},\n"
            "        account_only: {account_only},\n"
            "        applies_to_concluded: {applies_to_concluded},\n"
            "        account_allows: {account_allows},\n"
            "        restrict_future_enrollments: {restrict_future_enrollments},\n"
            "        not_for_masquerading: {not_for_masquerading},\n"
            "        deprecated: {deprecated},\n"
            "    }},".format(
                name=rust_str(name),
                label=rust_str(permission["label"]),
                true_for=rust_roles(permission["true_for"]),
                false_for=rust_roles(permission["false_for"]),
                available_to=rust_roles(word_list(block, "available_to") or []),
                account_only=account_only,
                applies_to_concluded=concluded_expr,
                account_allows="AccountAllows::Required"
                if "account_allows:" in block
                else "AccountAllows::Always",
                restrict_future_enrollments="true"
                if "restrict_future_enrollments: true" in block
                else "false",
                not_for_masquerading="true" if permission.get("not_for_masquerading") else "false",
                deprecated="true" if permission.get("deprecated") else "false",
            )
        )

    header = open(os.path.join(HERE, "registry_header.rs.txt")).read()
    with open(OUT_RS, "w") as handle:
        handle.write(header)
        handle.write("\n".join(out))
        handle.write("\n];\n\n")
        handle.write(
            "/// Look a permission up by name.\n"
            "pub fn lookup(name: &str) -> Option<&'static PermissionSpec> {\n"
            "    PERMISSIONS.iter().find(|spec| spec.name == name)\n"
            "}\n\n"
            "/// Whether Canvas declares a permission with this name.\n"
            "pub fn is_known(name: &str) -> bool {\n"
            "    lookup(name).is_some()\n"
            "}\n"
        )

    print("wrote %s (%d permissions)" % (OUT_RS, len(out)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
