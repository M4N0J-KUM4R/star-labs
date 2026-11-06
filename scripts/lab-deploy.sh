#!/usr/bin/env bash
set -euo pipefail

HOST="${LAB_SSH_HOST:-star-labs}"
SOURCE_DIR="${STAR_LMS_SOURCE_DIR:-}"
SOURCE_ROOT=""
SOURCE_PREFIX=""
DAHL_DIR="${DAHL_PROXY_DIR:-$HOME/projects/dahl-proxy}"
DAHL_HOME="${DAHL_HOME:-$HOME/.dahl-proxy}"
HERMES_HOME="${HERMES_HOME:-$HOME/.hermes}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

[[ -d "$DAHL_DIR" && -f "$DAHL_DIR/Cargo.toml" ]] || {
  echo 'DAHL_PROXY_DIR must contain the Dahl proxy Cargo.toml.' >&2; exit 1;
}
[[ -f "$DAHL_DIR/config.yaml" ]] || { echo 'Dahl config.yaml is missing.' >&2; exit 1; }
[[ -f "$DAHL_HOME/accounts.json" ]] || { echo 'Dahl account cache is missing in DAHL_HOME.' >&2; exit 1; }
[[ -f "$HERMES_HOME/config.yaml" && -f "$HERMES_HOME/.env" ]] || {
  echo 'Hermes config.yaml and .env are required in HERMES_HOME.' >&2; exit 1;
}

SOURCE_SHA=""
if [[ -n "$SOURCE_DIR" ]]; then
  [[ -d "$SOURCE_DIR" && -f "$SOURCE_DIR/Cargo.toml" ]] || {
    echo 'STAR_LMS_SOURCE_DIR must point to a Rust workspace directory containing Cargo.toml.' >&2; exit 1;
  }
  SOURCE_ROOT="$(git -C "$SOURCE_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
  [[ -n "$SOURCE_ROOT" ]] || { echo 'STAR_LMS_SOURCE_DIR must be inside a Git checkout.' >&2; exit 1; }
  SOURCE_PREFIX="$(git -C "$SOURCE_DIR" rev-parse --show-prefix)"
  if [[ -n "$(git -C "$SOURCE_ROOT" status --porcelain --untracked-files=all)" ]]; then
    echo 'Rust migration checkout is dirty; use a verified clean source checkout before deployment.' >&2
    exit 1
  fi
  if git -C "$SOURCE_ROOT" ls-files "$SOURCE_PREFIX" | grep -Eiq '(^|/)(\.env|.*\.pem|.*\.key|config\.ya?ml)$'; then
    echo 'Rust migration checkout tracks a likely secret/config file; inspect it before deployment.' >&2
    exit 1
  fi
  SOURCE_SHA="$(git -C "$SOURCE_ROOT" rev-parse --short HEAD)"
fi

if ! ssh "$HOST" 'sudo cloud-init status --wait'; then
  ssh "$HOST" 'test -f /var/log/star-labs-bootstrap.complete' || {
    echo 'Instance bootstrap did not complete. Inspect /var/log/star-labs-bootstrap.log before deploying.' >&2
    exit 1
  }
fi
ssh "$HOST" 'mkdir -p "$HOME/work/canvas-lms" "$HOME/work/dahl-proxy" "$HOME/.hermes" "$HOME/.dahl-proxy"'

# git archive sends committed source only; local edits and untracked files stay local.
if [[ -n "$SOURCE_DIR" ]]; then
  ssh "$HOST" 'mkdir -p "$HOME/work/canvas-lms"; find "$HOME/work/canvas-lms" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +'
  if [[ -n "$SOURCE_PREFIX" ]]; then
    git -C "$SOURCE_ROOT" archive --format=tar HEAD "$SOURCE_PREFIX" | ssh "$HOST" \
      'tar -xf - --strip-components=1 -C "$HOME/work/canvas-lms"'
  else
    git -C "$SOURCE_ROOT" archive --format=tar HEAD | ssh "$HOST" \
      'tar -xf - -C "$HOME/work/canvas-lms"'
  fi
fi
rsync -az --delete --exclude .git/ --exclude target/ --exclude config.yaml \
  --exclude 'config.yaml.*' "$DAHL_DIR/" "$HOST:work/dahl-proxy/"

# Credentials cross only SSH and remain outside the public GitHub repository.
scp "$DAHL_DIR/config.yaml" "$HOST:work/dahl-proxy/config.yaml.private"
scp "$DAHL_HOME/accounts.json" "$HOST:.dahl-proxy/accounts.json"
if [[ -f "$DAHL_HOME/client-api-key" ]]; then
  scp "$DAHL_HOME/client-api-key" "$HOST:.dahl-proxy/client-api-key"
fi
scp "$HERMES_HOME/config.yaml" "$HERMES_HOME/.env" "$HOST:.hermes/"
if [[ -f "$HERMES_HOME/auth.json" ]]; then
  scp "$HERMES_HOME/auth.json" "$HOST:.hermes/"
fi
scp "$REPO_ROOT/infra/dahl-proxy.service" "$HOST:/tmp/dahl-proxy.service"
ssh "$HOST" 'chmod 700 "$HOME/.hermes" "$HOME/.dahl-proxy"; chmod 600 "$HOME/.hermes/config.yaml" "$HOME/.hermes/.env" "$HOME/.dahl-proxy/accounts.json"; if [ -f "$HOME/.hermes/auth.json" ]; then chmod 600 "$HOME/.hermes/auth.json"; fi; if [ -f "$HOME/.dahl-proxy/client-api-key" ]; then chmod 600 "$HOME/.dahl-proxy/client-api-key"; fi'

ssh "$HOST" 'source "$HOME/.cargo/env"; export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; cd "$HOME/work/dahl-proxy"; CARGO_BUILD_JOBS=1 cargo build --release; sudo install -D -m 0755 target/release/dahl-proxy /usr/local/bin/dahl-proxy; sudo install -d -o root -g ec2-user -m 0750 /etc/dahl-proxy; if grep -q "^bind:" config.yaml.private; then sed -E "s/^bind:.*/bind: 127.0.0.1/" config.yaml.private > /tmp/dahl-config.yaml; else { printf "bind: 127.0.0.1\\n"; cat config.yaml.private; } > /tmp/dahl-config.yaml; fi; sudo install -o root -g ec2-user -m 0640 /tmp/dahl-config.yaml /etc/dahl-proxy/config.yaml; rm -f config.yaml.private /tmp/dahl-config.yaml; sudo install -o root -g root -m 0644 /tmp/dahl-proxy.service /etc/systemd/system/dahl-proxy.service; sudo systemctl daemon-reload; sudo systemctl enable --now dahl-proxy; sudo systemctl is-active --quiet dahl-proxy; hermes --version'

if [[ -n "$SOURCE_SHA" ]]; then
  printf 'Deployed committed source %s and started the loopback-only Dahl proxy on %s.\n' "$SOURCE_SHA" "$HOST"
else
  printf 'Configured Hermes and started the loopback-only Dahl proxy on %s; migration source was skipped.\n' "$HOST"
fi
