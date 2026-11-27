#!/usr/bin/env python3
"""Redact the recording token out of the committed corpus, and (optionally)
inject a live token for a replay run.

Why: the corpus is committed, so the Canvas access token used while
recording must never be in it. The corpus stores a placeholder instead:

    Authorization = "Bearer __REPLAY_TOKEN__"

A replay run substitutes a real token for the placeholder, runs the replay,
and puts the placeholder back. The token never touches the repository.

The Rust side authenticates with a JWT (star-auth::create_token), so the
injected token must be an HS256 JWT with the claims Rust expects:

    {"sub": <user_id>, "role": "<role>", "exp": <unix>}

Usage:
    token_tool.py redact  <case-dir>            # placeholder in, token out
    token_tool.py inject  <case-dir> <jwt>      # placeholder -> jwt
    token_tool.py restore <case-dir>            # jwt -> placeholder
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import sys
import time
from pathlib import Path

PLACEHOLDER = "Bearer __REPLAY_TOKEN__"
HEADER = {"alg": "HS256", "typ": "JWT"}


def make_jwt(user_id: int, role: str, secret: str, ttl: int = 86_400) -> str:
    """Mint an HS256 JWT with the claims star-auth expects."""

    def b64(raw: bytes) -> str:
        return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")

    header = b64(json.dumps(HEADER, separators=(",", ":")).encode())
    payload = b64(
        json.dumps(
            {"sub": user_id, "role": role, "exp": int(time.time()) + ttl},
            separators=(",", ":"),
        ).encode()
    )
    signing_input = f"{header}.{payload}".encode("ascii")
    signature = hmac.new(secret.encode("utf-8"), signing_input, hashlib.sha256).digest()
    return f"{header}.{payload}.{b64(signature)}"


def case_files(case_dir: Path) -> list[Path]:
    return sorted(case_dir.glob("*.toml"))


def redact(case_dir: Path) -> int:
    """Replace any concrete bearer token with the placeholder."""
    changed = 0
    for path in case_files(case_dir):
        text = path.read_text(encoding="utf-8")
        new = _swap_bearer(text, PLACEHOLDER)
        if new != text:
            path.write_text(new, encoding="utf-8")
            changed += 1
    return changed


def inject(case_dir: Path, token: str) -> int:
    changed = 0
    for path in case_files(case_dir):
        text = path.read_text(encoding="utf-8")
        new = text.replace(PLACEHOLDER, f"Bearer {token}")
        if new != text:
            path.write_text(new, encoding="utf-8")
            changed += 1
    return changed


def restore(case_dir: Path) -> int:
    changed = 0
    for path in case_files(case_dir):
        text = path.read_text(encoding="utf-8")
        new = _swap_bearer(text, PLACEHOLDER)
        if new != text:
            path.write_text(new, encoding="utf-8")
            changed += 1
    return changed


def _swap_bearer(text: str, replacement: str) -> str:
    """Replace the value of any Authorization header with `replacement`."""
    out = []
    for line in text.splitlines(keepends=True):
        stripped = line.strip()
        if stripped.startswith("Authorization"):
            indent = line[: len(line) - len(line.lstrip())]
            key = stripped.split("=", 1)[0].strip()
            out.append(f'{indent}{key} = "{replacement}"\n')
        else:
            out.append(line)
    return "".join(out)


def mint_with_server(args) -> str | None:
    """Mint a token with the app's own `star_auth::create_token`.

    The helper binary is `crates/star-http/src/bin/mint_token.rs`. It must be
    built first: `cargo build -p star-http --bin mint_token`.
    """
    import shutil
    import subprocess

    binary = shutil.which("mint_token")
    if binary is None:
        for candidate in (
            Path("target/debug/mint_token"),
            Path("target/release/mint_token"),
        ):
            if candidate.is_file() and os.access(candidate, os.X_OK):
                binary = str(candidate)
                break
    if binary is None:
        return None
    env = dict(os.environ)
    if args.secret:
        env["STAR_JWT_SECRET"] = args.secret
    try:
        out = subprocess.run(
            [binary, str(args.user_id), args.role],
            capture_output=True, text=True, env=env, timeout=30,
        )
    except Exception:
        return None
    if out.returncode != 0:
        return None
    token = out.stdout.strip()
    return token or None


def seed_replay_token(args) -> int:
    """Create the access_tokens + developer_keys rows the replay needs.

    The recorded corpus carries Canvas' `x-canvas-meta` values, which name
    the access token and the developer key by their global ids. The port can
    only reproduce those if it resolves the caller from an `access_tokens`
    row — which is Canvas' model — so the replay database needs one.

    The ids must match what the corpus recorded: access token 7
    (`at=10000000000007`) and developer key 1 (`dk=10000000000001`).
    """
    import subprocess
    import tempfile

    url = os.environ.get("DATABASE_URL")
    if not url:
        print("token_tool: DATABASE_URL must be set to seed the replay token",
              file=sys.stderr)
        return 2
    if not args.secret:
        print("token_tool: --secret (or STAR_JWT_SECRET) must be set", file=sys.stderr)
        return 2

    token = args.token or "parity-replay-access-token"
    digest = make_jwt.__globals__["hmac"].new(
        args.secret.encode(), token.encode(), "sha1"
    ).hexdigest()

    sql = f"""
    INSERT INTO developer_keys
        (id, name, workflow_state, api_key, user_id, scopes, created_at, updated_at)
    VALUES (1, 'Parity Key', 'active',
            'paritykey0000000000000000000000000000000000000000000000000000000',
            {args.user_id}, '{{}}'::text[], now(), now())
    ON CONFLICT (id) DO NOTHING;

    INSERT INTO access_tokens
        (id, user_id, developer_key_id, token, scopes, purpose, workflow_state,
         permanent_expires_at, created_at, updated_at, token_hint, crypted_token)
    VALUES ({args.token_id}, {args.user_id}, 1, '{token}', '{{}}'::text[],
            'Parity parity', 'active', now() + interval '10 years',
            now(), now(), '{token[:5]}', '{digest}')
    ON CONFLICT (id) DO UPDATE
       SET crypted_token = EXCLUDED.crypted_token,
           permanent_expires_at = EXCLUDED.permanent_expires_at,
           workflow_state = 'active';
    """
    with tempfile.NamedTemporaryFile("w", suffix=".sql", delete=False) as fh:
        fh.write(sql)
        path = fh.name
    try:
        out = subprocess.run(
            ["psql", url, "-v", "ON_ERROR_STOP=1", "-f", path],
            capture_output=True, text=True,
        )
    except FileNotFoundError:
        print("token_tool: psql is not on PATH", file=sys.stderr)
        return 2
    finally:
        os.unlink(path)
    if out.returncode != 0:
        print(f"token_tool: seeding failed: {out.stderr.strip()}", file=sys.stderr)
        return 1
    print(f"seeded access_tokens id {args.token_id} and developer_keys id 1")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["redact", "inject", "restore", "seed"])
    ap.add_argument("case_dir", type=Path, nargs="?")
    ap.add_argument("token", nargs="?", default=None)
    ap.add_argument("--user-id", type=int, default=1)
    ap.add_argument("--token-id", type=int, default=7,
                    help="the access_tokens row id the recorded corpus expects")
    ap.add_argument("--role", default="teacher")
    ap.add_argument("--secret", default=os.environ.get("STAR_JWT_SECRET", ""))
    ap.add_argument("--access-token", default=None,
                    help="a Canvas-style access token (an access_tokens row) "
                         "instead of the port's own session token; this is what "
                         "reproduces Canvas' x-canvas-meta at=/dk= values")
    args = ap.parse_args()

    if args.action == "seed":
        return seed_replay_token(args)
    if args.case_dir is None:
        print("token_tool: case_dir is required for "
              + args.action, file=sys.stderr)
        return 2

    if args.action == "inject":
        if args.access_token:
            args.token = args.access_token
        if not args.token:
            # Prefer the server's own signing code (star_auth::create_token)
            # over a re-implementation of the JWT here: the replay and the
            # server must never disagree about the token format.
            minted = mint_with_server(args)
            if minted is None:
                if not args.secret:
                    print("token_tool: need a token or --secret/STAR_JWT_SECRET", file=sys.stderr)
                    return 2
                minted = make_jwt(args.user_id, args.role, args.secret)
            args.token = minted
        n = inject(args.case_dir, args.token)
        print(f"injected into {n} case files")
        return 0

    fn = {"redact": redact, "restore": restore}[args.action]
    n = fn(args.case_dir)
    print(f"{args.action}: {n} case files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
