#!/usr/bin/env python3
"""Check that the cutover front door delegates what the port does not own.

The front door is the cutover mechanism: with `STAR_CANVAS_ORIGIN` set, the
port answers the routes it owns and sends the rest to the Canvas instance
the deployment still runs. This starts the port against a mock upstream and
checks both halves:

* a path the port does not own comes from the upstream, headers and body
  and all;
* a path the port does own is answered by the port itself.

Without an origin the port must not delegate: the same path answers 404 and
the upstream is never called.

Usage:
    front_door_check.py --binary target/release/star-http --database-url ...
"""
from __future__ import annotations

import argparse
import http.server
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

MARKER_HEADER = "X-Upstream-Marker"
MARKER_VALUE = "the-front-door-delegated-this"
PROBE_PATH = "/api/v1/front_door_probe/does-not-exist"
OWNED_PATH = "/health"


class MockCanvas(http.server.BaseHTTPRequestHandler):
    """A stand-in for the Canvas instance the front door delegates to."""

    def do_GET(self):  # noqa: N802 - the http.server API
        body = json.dumps({"delegated": self.path}).encode()
        self.send_response(200)
        self.send_header(MARKER_HEADER, MARKER_VALUE)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):  # noqa: A003 - silence the access log
        pass


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(url: str, timeout: float = 60.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2):
                return True
        except urllib.error.HTTPError:
            return True
        except Exception:
            time.sleep(0.5)
    return False


def get(url: str, headers: dict[str, str] | None = None):
    """Fetch a URL and return its status, its headers and its body.

    The headers are the message itself, not a plain dict: HTTP header names
    are case-insensitive and the proxy writes them lowercased, so a lookup
    has to be case-insensitive too.
    """
    request = urllib.request.Request(url, headers=headers or {})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, response.headers, response.read().decode()
    except urllib.error.HTTPError as error:
        return error.code, error.headers, error.read().decode()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--database-url", required=True)
    parser.add_argument("--jwt-secret", required=True)
    args = parser.parse_args()

    upstream_port = free_port()
    upstream = http.server.ThreadingHTTPServer(("127.0.0.1", upstream_port), MockCanvas)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()

    port = free_port()
    env = dict(os.environ)
    env["DATABASE_URL"] = args.database_url
    env["STAR_JWT_SECRET"] = args.jwt_secret
    env["STAR_PORT"] = str(port)
    env["STAR_CANVAS_ORIGIN"] = f"http://127.0.0.1:{upstream_port}"
    server = subprocess.Popen(
        [args.binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    failures: list[str] = []
    try:
        if not wait_for(f"http://127.0.0.1:{port}/health"):
            print("front door: the port did not start", file=sys.stderr)
            return 1

        # 1. a path the port does not own comes from the upstream.
        status, headers, body = get(f"http://127.0.0.1:{port}{PROBE_PATH}")
        if headers.get(MARKER_HEADER) != MARKER_VALUE:
            failures.append(
                f"the port did not delegate {PROBE_PATH} (status {status}, body {body[:120]!r})"
            )
        elif json.loads(body).get("delegated") != PROBE_PATH:
            failures.append(f"the upstream answered for another path: {body[:120]!r}")

        # 2. a path the port does own is answered by the port.
        status, headers, body = get(f"http://127.0.0.1:{port}{OWNED_PATH}")
        if headers.get(MARKER_HEADER) == MARKER_VALUE:
            failures.append(f"the port delegated {OWNED_PATH}, which it owns")

        # 3. without an origin the port does not delegate.
        env.pop("STAR_CANVAS_ORIGIN")
        plain_port = free_port()
        env["STAR_PORT"] = str(plain_port)
        plain = subprocess.Popen(
            [args.binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        try:
            if not wait_for(f"http://127.0.0.1:{plain_port}/health"):
                failures.append("the port did not start without an origin")
            else:
                status, headers, body = get(f"http://127.0.0.1:{plain_port}{PROBE_PATH}")
                if headers.get(MARKER_HEADER) == MARKER_VALUE:
                    failures.append("the port delegated without an origin")
                elif status != 404:
                    failures.append(
                        f"without an origin the port answered {status}, expected 404"
                    )
        finally:
            plain.terminate()
            plain.wait(timeout=10)
    finally:
        server.terminate()
        server.wait(timeout=10)
        upstream.shutdown()

    for failure in failures:
        print(f"front door: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("front door: the port delegates what it does not own and keeps what it does")
    return 0


if __name__ == "__main__":
    sys.exit(main())
