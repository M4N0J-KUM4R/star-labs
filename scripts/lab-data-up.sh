#!/usr/bin/env bash
set -euo pipefail

PROFILE="${AWS_PROFILE:-}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-$(aws configure get region --profile "${PROFILE:-default}" 2>/dev/null || true)}}"
REGION="${REGION:-us-west-2}"
STACK_NAME="${STAR_LABS_STACK_NAME:-star-labs-data}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PARAM_FILE=""

aws_lab() {
  if [[ -n "$PROFILE" ]]; then
    command aws --profile "$PROFILE" "$@"
  else
    command aws "$@"
  fi
}
cleanup() {
  [[ -z "$PARAM_FILE" ]] || rm -f "$PARAM_FILE"
}
trap cleanup EXIT

ACCOUNT_ID="$(aws_lab sts get-caller-identity --query Account --output text)"
VPC_ID="$(aws_lab ec2 describe-vpcs --region "$REGION" --filters Name=isDefault,Values=true \
  --query 'Vpcs[0].VpcId' --output text)"
[[ "$VPC_ID" != None && -n "$VPC_ID" ]] || { echo "No default VPC in $REGION" >&2; exit 1; }
EC2_SG="$(aws_lab ec2 describe-instances --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
    Name=tag:Name,Values=star-labs-lab \
    Name=instance-state-name,Values=running \
  --query 'Reservations[0].Instances[0].SecurityGroups[0].GroupId' --output text)"
[[ "$EC2_SG" != None && -n "$EC2_SG" ]] || { echo 'Run scripts/lab-up.sh first.' >&2; exit 1; }
EC2_ID="$(aws_lab ec2 describe-instances --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
    Name=tag:Name,Values=star-labs-lab \
    Name=instance-state-name,Values=running \
  --query 'Reservations[0].Instances[0].InstanceId' --output text)"

SUBNETS="$(aws_lab ec2 describe-subnets --region "$REGION" --filters "Name=vpc-id,Values=$VPC_ID" \
  --query 'sort_by(Subnets,&AvailabilityZone)[].{Id:SubnetId,Az:AvailabilityZone}' --output json)"
read -r DB_SUBNET_A DB_SUBNET_B < <(python3 - "$SUBNETS" <<'PY'
import json, sys
subnets = json.loads(sys.argv[1])
seen, selected = set(), []
for subnet in subnets:
    if subnet['Az'] not in seen:
        selected.append(subnet['Id'])
        seen.add(subnet['Az'])
    if len(selected) == 2:
        break
if len(selected) != 2:
    raise SystemExit('RDS requires default subnets in two Availability Zones')
print(*selected)
PY
)

DB_PASSWORD_FILE="$HOME/.config/star-labs/db-password"
mkdir -p "$(dirname "$DB_PASSWORD_FILE")"
chmod 700 "$(dirname "$DB_PASSWORD_FILE")"
if [[ ! -s "$DB_PASSWORD_FILE" ]]; then
  python3 - <<'PY' > "$DB_PASSWORD_FILE"
import secrets, string
alphabet = string.ascii_letters + string.digits + '_+=.!#$*-'
print(''.join(secrets.choice(alphabet) for _ in range(32)) + 'A1!')
PY
fi
chmod 600 "$DB_PASSWORD_FILE"
DB_PASSWORD="$(cat "$DB_PASSWORD_FILE")"
PARAM_FILE="$(mktemp "${TMPDIR:-/tmp}/star-labs-cfn-params.XXXXXX")"
chmod 600 "$PARAM_FILE"
python3 - "$PARAM_FILE" "$VPC_ID" "$EC2_SG" "$EC2_ID" "$DB_SUBNET_A" "$DB_SUBNET_B" "$DB_PASSWORD" <<'PY'
import json, sys
path, vpc, ec2_sg, ec2_id, subnet_a, subnet_b, password = sys.argv[1:]
parameters = [
    {'ParameterKey': 'VpcId', 'ParameterValue': vpc},
    {'ParameterKey': 'Ec2SecurityGroupId', 'ParameterValue': ec2_sg},
    {'ParameterKey': 'Ec2InstanceId', 'ParameterValue': ec2_id},
    {'ParameterKey': 'DbSubnetA', 'ParameterValue': subnet_a},
    {'ParameterKey': 'DbSubnetB', 'ParameterValue': subnet_b},
    {'ParameterKey': 'DbMasterPassword', 'ParameterValue': password},
]
with open(path, 'w') as f:
    json.dump(parameters, f)
PY
unset DB_PASSWORD

if aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" >/dev/null 2>&1; then
  STACK_STATUS="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
    --query 'Stacks[0].StackStatus' --output text)"
  if [[ "$STACK_STATUS" == CREATE_IN_PROGRESS ]]; then
    aws_lab cloudformation wait stack-create-complete --region "$REGION" --stack-name "$STACK_NAME"
    STACK_STATUS="CREATE_COMPLETE"
  elif [[ "$STACK_STATUS" == UPDATE_IN_PROGRESS ]]; then
    aws_lab cloudformation wait stack-update-complete --region "$REGION" --stack-name "$STACK_NAME"
    STACK_STATUS="UPDATE_COMPLETE"
  fi
  if [[ "$STACK_STATUS" == ROLLBACK_COMPLETE || "$STACK_STATUS" == CREATE_FAILED ]]; then
    aws_lab cloudformation delete-stack --region "$REGION" --stack-name "$STACK_NAME"
    aws_lab cloudformation wait stack-delete-complete --region "$REGION" --stack-name "$STACK_NAME"
    STACK_STATUS=""
  fi
else
  STACK_STATUS=""
fi
if [[ -n "$STACK_STATUS" ]]; then
  UPDATE_OUTPUT=""
  if UPDATE_OUTPUT="$(aws_lab cloudformation update-stack --region "$REGION" --stack-name "$STACK_NAME" \
    --template-body "file://$REPO_ROOT/infra/lab-data-stack.yaml" --parameters "file://$PARAM_FILE" 2>&1)"; then
    aws_lab cloudformation wait stack-update-complete --region "$REGION" --stack-name "$STACK_NAME"
  elif grep -q 'No updates are to be performed' <<<"${UPDATE_OUTPUT:-}"; then
    printf 'Stack %s already matches the template.\n' "$STACK_NAME"
  else
    printf '%s\n' "$UPDATE_OUTPUT" >&2
    exit 1
  fi
else
  aws_lab cloudformation create-stack --region "$REGION" --stack-name "$STACK_NAME" \
    --template-body "file://$REPO_ROOT/infra/lab-data-stack.yaml" --parameters "file://$PARAM_FILE"
  aws_lab cloudformation wait stack-create-complete --region "$REGION" --stack-name "$STACK_NAME"
fi

aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[].[OutputKey,OutputValue]' --output table

DB_ENDPOINT="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[?OutputKey==`DatabaseEndpoint`].OutputValue|[0]' --output text)"
DB_PORT="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[?OutputKey==`DatabasePort`].OutputValue|[0]' --output text)"
BUCKET_NAME="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[?OutputKey==`BucketName`].OutputValue|[0]' --output text)"
DATA_ENV="$HOME/.config/star-labs/data.env"
python3 - "$DATA_ENV" "$DB_ENDPOINT" "$DB_PORT" "$BUCKET_NAME" "$REGION" "$DB_PASSWORD_FILE" <<'PY'
from pathlib import Path
from urllib.parse import quote
import sys
path, endpoint, port, bucket, region, password_file = sys.argv[1:]
password = Path(password_file).read_text().strip()
database_url = f"postgresql://staradmin:{quote(password, safe='')}@{endpoint}:{port}/star_lms?sslmode=require"
content = f"DATABASE_URL={database_url}\nS3_BUCKET={bucket}\nAWS_REGION={region}\n"
Path(path).write_text(content)
Path(path).chmod(0o600)
PY

SSH_HOST="${LAB_SSH_HOST:-star-labs}"
if ssh -o BatchMode=yes -o ConnectTimeout=5 "$SSH_HOST" true >/dev/null 2>&1; then
  ssh "$SSH_HOST" 'mkdir -p "$HOME/.config/star-labs" && chmod 700 "$HOME/.config/star-labs"'
  scp "$DATA_ENV" "$SSH_HOST:.config/star-labs/data.env"
  ssh "$SSH_HOST" 'chmod 600 "$HOME/.config/star-labs/data.env"'
fi
printf 'Account: %s\nRegion: %s\nDB env file (mode 600): %s\nS3 bucket: %s\n' \
  "$ACCOUNT_ID" "$REGION" "$DATA_ENV" "$BUCKET_NAME"
