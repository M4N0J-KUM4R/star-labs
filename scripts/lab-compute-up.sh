#!/usr/bin/env bash
set -euo pipefail

PROFILE="${AWS_PROFILE:-}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-$(aws configure get region --profile "${PROFILE:-default}" 2>/dev/null || true)}}"
REGION="${REGION:-us-west-2}"
STACK_NAME="${STAR_LABS_COMPUTE_STACK_NAME:-star-labs-compute}"
SSH_CIDR="${SSH_CIDR:-}"
KEY_NAME="star-labs-lab"
KEY_FILE="$HOME/.ssh/${KEY_NAME}.pem"
AMI_PARAMETER="/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PARAM_FILE=""

aws_lab() {
  if [[ -n "$PROFILE" ]]; then
    command aws --profile "$PROFILE" "$@"
  else
    command aws "$@"
  fi
}
cleanup() { [[ -z "$PARAM_FILE" ]] || rm -f "$PARAM_FILE"; }
trap cleanup EXIT

command -v aws >/dev/null || { echo 'aws CLI is required' >&2; exit 1; }
command -v curl >/dev/null || { echo 'curl is required' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'python3 is required' >&2; exit 1; }
command -v ssh-keygen >/dev/null || { echo 'ssh-keygen is required' >&2; exit 1; }

ACCOUNT_ID="$(aws_lab sts get-caller-identity --query Account --output text)"
[[ "$ACCOUNT_ID" =~ ^[0-9]{12}$ ]] || { echo 'Could not determine the active AWS account' >&2; exit 1; }
RUNNING_COUNT="$(aws_lab ec2 describe-instances --region "$REGION" \
  --filters Name=instance-state-name,Values=pending,running \
  --query 'length(Reservations[].Instances[])' --output text)"
(( RUNNING_COUNT < 9 )) || { echo 'The lab limit of nine running EC2 instances is reached.' >&2; exit 1; }

if [[ -z "$SSH_CIDR" ]]; then
  OPERATOR_IP="$(curl -fsS --max-time 8 https://checkip.amazonaws.com | tr -d '\r\n')"
  SSH_CIDR="${OPERATOR_IP}/32"
fi
python3 - "$SSH_CIDR" <<'PY'
import ipaddress, sys
network = ipaddress.ip_network(sys.argv[1], strict=False)
if network.version != 4 or network.prefixlen != 32:
    raise SystemExit('SSH_CIDR must be one IPv4 address ending in /32')
PY

AMI_ID="$(aws_lab ssm get-parameter --region "$REGION" --name "$AMI_PARAMETER" \
  --query 'Parameter.Value' --output text)"
VPC_ID="$(aws_lab ec2 describe-vpcs --region "$REGION" \
  --filters Name=isDefault,Values=true --query 'Vpcs[0].VpcId' --output text)"
[[ "$VPC_ID" != None && -n "$VPC_ID" ]] || { echo "No default VPC in $REGION" >&2; exit 1; }
SUBNET_ID="$(aws_lab ec2 describe-subnets --region "$REGION" \
  --filters "Name=vpc-id,Values=$VPC_ID" Name=default-for-az,Values=true \
  --query 'Subnets[?MapPublicIpOnLaunch==`true`]|[0].SubnetId' --output text)"
[[ "$SUBNET_ID" != None && -n "$SUBNET_ID" ]] || { echo "No public default subnet in $REGION" >&2; exit 1; }

mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
if aws_lab ec2 describe-key-pairs --region "$REGION" --key-names "$KEY_NAME" >/dev/null 2>&1; then
  [[ -r "$KEY_FILE" ]] || {
    echo "Key pair $KEY_NAME exists in AWS but $KEY_FILE is missing; restore it or use a fresh lab." >&2
    exit 1
  }
else
  if [[ -e "$KEY_FILE" ]]; then
    mv "$KEY_FILE" "${KEY_FILE}.previous.$(date +%Y%m%d%H%M%S)"
  fi
  umask 077
  aws_lab ec2 create-key-pair --region "$REGION" --key-name "$KEY_NAME" \
    --key-type ed25519 --key-format pem --query KeyMaterial --output text > "$KEY_FILE"
fi
chmod 600 "$KEY_FILE"

PARAM_FILE="$(mktemp "${TMPDIR:-/tmp}/star-labs-compute-params.XXXXXX")"
chmod 600 "$PARAM_FILE"
python3 - "$PARAM_FILE" "$VPC_ID" "$AMI_ID" "$SUBNET_ID" "$KEY_NAME" "$SSH_CIDR" <<'PY'
import json, sys
path, vpc, ami, subnet, key, cidr = sys.argv[1:]
parameters = [
    {'ParameterKey': 'VpcId', 'ParameterValue': vpc},
    {'ParameterKey': 'AmiId', 'ParameterValue': ami},
    {'ParameterKey': 'SubnetId', 'ParameterValue': subnet},
    {'ParameterKey': 'KeyName', 'ParameterValue': key},
    {'ParameterKey': 'SshCidr', 'ParameterValue': cidr},
    {'ParameterKey': 'InstanceProfileName', 'ParameterValue': 'LabInstanceProfile'},
]
with open(path, 'w') as file:
    json.dump(parameters, file)
PY

TEMPLATE="$REPO_ROOT/infra/lab-compute-stack.yaml"
STACK_STATUS=""
if aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" >/dev/null 2>&1; then
  STACK_STATUS="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
    --query 'Stacks[0].StackStatus' --output text)"
  case "$STACK_STATUS" in
    CREATE_IN_PROGRESS) aws_lab cloudformation wait stack-create-complete --region "$REGION" --stack-name "$STACK_NAME"; STACK_STATUS=CREATE_COMPLETE ;;
    UPDATE_IN_PROGRESS) aws_lab cloudformation wait stack-update-complete --region "$REGION" --stack-name "$STACK_NAME"; STACK_STATUS=UPDATE_COMPLETE ;;
    DELETE_IN_PROGRESS) aws_lab cloudformation wait stack-delete-complete --region "$REGION" --stack-name "$STACK_NAME"; STACK_STATUS="" ;;
    ROLLBACK_COMPLETE|CREATE_FAILED)
      aws_lab cloudformation delete-stack --region "$REGION" --stack-name "$STACK_NAME"
      aws_lab cloudformation wait stack-delete-complete --region "$REGION" --stack-name "$STACK_NAME"
      STACK_STATUS=""
      ;;
  esac
fi

if [[ -z "$STACK_STATUS" ]]; then
  aws_lab cloudformation create-stack --region "$REGION" --stack-name "$STACK_NAME" \
    --template-body "file://$TEMPLATE" --parameters "file://$PARAM_FILE"
  aws_lab cloudformation wait stack-create-complete --region "$REGION" --stack-name "$STACK_NAME"
else
  if UPDATE_OUTPUT="$(aws_lab cloudformation update-stack --region "$REGION" --stack-name "$STACK_NAME" \
      --template-body "file://$TEMPLATE" --parameters "file://$PARAM_FILE" 2>&1)"; then
    aws_lab cloudformation wait stack-update-complete --region "$REGION" --stack-name "$STACK_NAME"
  elif grep -q 'No updates are to be performed' <<<"$UPDATE_OUTPUT"; then
    printf 'Stack %s already matches the template and parameters.\n' "$STACK_NAME"
  else
    printf '%s\n' "$UPDATE_OUTPUT" >&2
    exit 1
  fi
fi

INSTANCE_ID="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[?OutputKey==`InstanceId`].OutputValue|[0]' --output text)"
PUBLIC_IP="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
  --query 'Stacks[0].Outputs[?OutputKey==`PublicIp`].OutputValue|[0]' --output text)"
[[ -n "$INSTANCE_ID" && "$INSTANCE_ID" != None && -n "$PUBLIC_IP" && "$PUBLIC_IP" != None ]] || {
  echo 'CloudFormation completed without the expected instance outputs.' >&2; exit 1;
}

CONFIG_DIR="$HOME/.ssh/config.d"
CONFIG_FILE="$HOME/.ssh/config"
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [[ ! -f "$CONFIG_FILE" ]]; then : > "$CONFIG_FILE"; chmod 600 "$CONFIG_FILE"; fi
if ! grep -Fq 'Include ~/.ssh/config.d/*.conf' "$CONFIG_FILE"; then
  TEMP_CONFIG="$(mktemp)"
  printf '%s\n' 'Include ~/.ssh/config.d/*.conf' > "$TEMP_CONFIG"
  cat "$CONFIG_FILE" >> "$TEMP_CONFIG"
  cat "$TEMP_CONFIG" > "$CONFIG_FILE"
  rm -f "$TEMP_CONFIG"
  chmod 600 "$CONFIG_FILE"
fi
ssh-keygen -R star-labs >/dev/null 2>&1 || true
ssh-keygen -R star-lms >/dev/null 2>&1 || true
ssh-keygen -R "$PUBLIC_IP" >/dev/null 2>&1 || true
cat > "$CONFIG_DIR/star-labs-lab.conf" <<EOF
Host star-labs star-labs-lab star-lms
  HostName $PUBLIC_IP
  User ec2-user
  IdentityFile $KEY_FILE
  IdentitiesOnly yes
  StrictHostKeyChecking accept-new
EOF
chmod 600 "$CONFIG_DIR/star-labs-lab.conf"

printf 'Account: %s\nRegion: %s\nStack: %s\nInstance: %s (t3.medium)\nSSH source: %s\nConnect: ssh star-labs\n' \
  "$ACCOUNT_ID" "$REGION" "$STACK_NAME" "$INSTANCE_ID" "$SSH_CIDR"
