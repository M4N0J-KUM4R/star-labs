#!/usr/bin/env bash
set -euo pipefail

BASE_HOST="${LAB_BASE_SSH_HOST:-star-labs}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-west-2}}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REMOTE_DIR='work/bootstrap'

ssh "$BASE_HOST" "mkdir -p \"\$HOME/$REMOTE_DIR\""
scp "$REPO_ROOT/infra/user-data.sh" "$BASE_HOST:$REMOTE_DIR/agents-user-data.sh"
scp "$REPO_ROOT/infra/user-data-migration.sh" "$BASE_HOST:$REMOTE_DIR/migration-user-data.sh"
scp "$REPO_ROOT/scripts/lab-workers-remote.sh" "$BASE_HOST:$REMOTE_DIR/lab-workers-remote.sh"
RESULTS="$(ssh "$BASE_HOST" "chmod 700 \"\$HOME/$REMOTE_DIR/lab-workers-remote.sh\"; AWS_REGION='$REGION' bash \"\$HOME/$REMOTE_DIR/lab-workers-remote.sh\"")"

CONFIG_FILE="$HOME/.ssh/config"
CONFIG_DIR="$HOME/.ssh/config.d"
mkdir -p "$CONFIG_DIR"
chmod 700 "$HOME/.ssh" "$CONFIG_DIR"
if [[ ! -f "$CONFIG_FILE" ]]; then : > "$CONFIG_FILE"; chmod 600 "$CONFIG_FILE"; fi
if ! grep -Fq 'Include ~/.ssh/config.d/*.conf' "$CONFIG_FILE"; then
  TEMP_CONFIG="$(mktemp)"
  printf '%s\n' 'Include ~/.ssh/config.d/*.conf' > "$TEMP_CONFIG"
  cat "$CONFIG_FILE" >> "$TEMP_CONFIG"
  cat "$TEMP_CONFIG" > "$CONFIG_FILE"
  rm -f "$TEMP_CONFIG"
  chmod 600 "$CONFIG_FILE"
fi

WORKER_CONFIG="$CONFIG_DIR/star-labs-workers.conf"
: > "$WORKER_CONFIG"
while IFS=$'\t' read -r NAME INSTANCE_ID PUBLIC_IP; do
  [[ -n "$NAME" && -n "$PUBLIC_IP" && "$PUBLIC_IP" != None ]] || continue
  ssh-keygen -R "$NAME" >/dev/null 2>&1 || true
  ssh-keygen -R "$PUBLIC_IP" >/dev/null 2>&1 || true
  cat >> "$WORKER_CONFIG" <<EOF
Host $NAME
  HostName $PUBLIC_IP
  User ec2-user
  IdentityFile $HOME/.ssh/star-labs-lab.pem
  IdentitiesOnly yes
  StrictHostKeyChecking accept-new

EOF
  printf '%s\t%s\t%s\n' "$NAME" "$INSTANCE_ID" "$PUBLIC_IP"
done <<< "$RESULTS"
chmod 600 "$WORKER_CONFIG"
