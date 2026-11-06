#!/usr/bin/env bash
set -euo pipefail

PROFILE="${AWS_PROFILE:-}"
CONFIG_PROFILE="${PROFILE:-default}"
aws_lab() {
  if [[ -n "$PROFILE" ]]; then
    command aws --profile "$PROFILE" "$@"
  else
    command aws "$@"
  fi
}
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-$(aws configure get region --profile "$CONFIG_PROFILE" 2>/dev/null || true)}}"
REGION="${REGION:-us-west-2}"
SSH_CIDR="${SSH_CIDR:-}"
KEY_NAME="star-labs-lab"
KEY_FILE="$HOME/.ssh/${KEY_NAME}.pem"
AMI_PARAMETER="/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64"

command -v aws >/dev/null || { echo 'aws CLI is required' >&2; exit 1; }
command -v curl >/dev/null || { echo 'curl is required' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'python3 is required' >&2; exit 1; }
command -v ssh-keygen >/dev/null || { echo 'ssh-keygen is required' >&2; exit 1; }

ACCOUNT_ID="$(aws_lab sts get-caller-identity --query Account --output text)"
[[ "$ACCOUNT_ID" =~ ^[0-9]{12}$ ]] || { echo 'Could not determine the active AWS account' >&2; exit 1; }

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

AMI_ID="$(aws_lab ssm get-parameter --region "$REGION" \
  --name "$AMI_PARAMETER" --query 'Parameter.Value' --output text)"
VPC_ID="$(aws_lab ec2 describe-vpcs --region "$REGION" \
  --filters Name=isDefault,Values=true --query 'Vpcs[0].VpcId' --output text)"
[[ "$VPC_ID" != None && -n "$VPC_ID" ]] || { echo "No default VPC in $REGION" >&2; exit 1; }
SUBNET_ID="$(aws_lab ec2 describe-subnets --region "$REGION" \
  --filters "Name=vpc-id,Values=$VPC_ID" Name=default-for-az,Values=true \
  --query 'Subnets[?MapPublicIpOnLaunch==`true`]|[0].SubnetId' --output text)"
[[ "$SUBNET_ID" != None && -n "$SUBNET_ID" ]] || { echo "No public default subnet in $REGION" >&2; exit 1; }

mkdir -p "$HOME/.ssh"
chmod 700 "$HOME/.ssh"
if aws_lab ec2 describe-key-pairs --region "$REGION" \
  --key-names "$KEY_NAME" >/dev/null 2>&1; then
  [[ -r "$KEY_FILE" ]] || {
    echo "Key pair $KEY_NAME exists in AWS but $KEY_FILE is missing; remove/recreate it in the lab before continuing." >&2
    exit 1
  }
else
  umask 077
  aws_lab ec2 create-key-pair --region "$REGION" \
    --key-name "$KEY_NAME" --key-type ed25519 --key-format pem \
    --query KeyMaterial --output text > "$KEY_FILE"
fi
chmod 600 "$KEY_FILE"

SG_SUFFIX="${SSH_CIDR//./-}"
SG_SUFFIX="${SG_SUFFIX//\//-}"
SG_NAME="star-labs-ssh-${SG_SUFFIX}"
SG_ID="$(aws_lab ec2 describe-security-groups --region "$REGION" \
  --filters "Name=group-name,Values=$SG_NAME" "Name=vpc-id,Values=$VPC_ID" \
  --query 'SecurityGroups[0].GroupId' --output text)"
if [[ "$SG_ID" == None || -z "$SG_ID" ]]; then
  SG_ID="$(aws_lab ec2 create-security-group --region "$REGION" \
    --group-name "$SG_NAME" --description 'star-labs learner lab SSH restricted to operator /32' \
    --vpc-id "$VPC_ID" --query GroupId --output text)"
  aws_lab ec2 create-tags --region "$REGION" --resources "$SG_ID" \
    --tags Key=Project,Value=star-labs Key=Purpose,Value=learner-lab >/dev/null
fi
CURRENT_SSH_RULES="$(aws_lab ec2 describe-security-groups --region "$REGION" \
  --group-ids "$SG_ID" --query 'SecurityGroups[0].IpPermissions[?IpProtocol==`tcp` && FromPort==`22` && ToPort==`22`].IpRanges[].CidrIp' --output text)"
if ! grep -Fq "$SSH_CIDR" <<<"$CURRENT_SSH_RULES"; then
  aws_lab ec2 authorize-security-group-ingress --region "$REGION" \
    --group-id "$SG_ID" --protocol tcp --port 22 --cidr "$SSH_CIDR" >/dev/null
fi

EXISTING_INSTANCE="$(aws_lab ec2 describe-instances --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
    Name=instance-state-name,Values=pending,running \
  --query 'Reservations[0].Instances[0].InstanceId' --output text)"
if [[ -n "$EXISTING_INSTANCE" && "$EXISTING_INSTANCE" != None ]]; then
  INSTANCE_ID="$EXISTING_INSTANCE"
  INSTANCE_TYPE="$(aws_lab ec2 describe-instances --region "$REGION" \
    --instance-ids "$INSTANCE_ID" --query 'Reservations[0].Instances[0].InstanceType' --output text)"
  [[ "$INSTANCE_TYPE" == t3.medium ]] || {
    echo "Existing tagged instance $INSTANCE_ID is $INSTANCE_TYPE, expected t3.medium; refusing to reuse it." >&2
    exit 1
  }
  aws_lab ec2 modify-instance-attribute --region "$REGION" \
    --instance-id "$INSTANCE_ID" --groups "$SG_ID"
else
  INSTANCE_ID="$(aws_lab ec2 run-instances --region "$REGION" \
    --image-id "$AMI_ID" --instance-type t3.medium --key-name "$KEY_NAME" \
    --iam-instance-profile Name=LabInstanceProfile --subnet-id "$SUBNET_ID" \
    --security-group-ids "$SG_ID" --count 1 --associate-public-ip-address \
    --metadata-options HttpEndpoint=enabled,HttpTokens=required \
    --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":100,"VolumeType":"gp3","Encrypted":true,"DeleteOnTermination":true}}]' \
    --user-data "file://$(cd "$(dirname "$0")/.." && pwd)/infra/user-data.sh" \
    --tag-specifications \
      'ResourceType=instance,Tags=[{Key=Name,Value=star-labs-lab},{Key=Project,Value=star-labs},{Key=Purpose,Value=learner-lab}]' \
      'ResourceType=volume,Tags=[{Key=Name,Value=star-labs-lab-root},{Key=Project,Value=star-labs},{Key=Purpose,Value=learner-lab}]' \
    --query 'Instances[0].InstanceId' --output text)"
fi

aws_lab ec2 wait instance-running --region "$REGION" --instance-ids "$INSTANCE_ID"
PUBLIC_IP="$(aws_lab ec2 describe-instances --region "$REGION" \
  --instance-ids "$INSTANCE_ID" --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)"
[[ "$PUBLIC_IP" != None && -n "$PUBLIC_IP" ]] || { echo 'Instance has no public IP' >&2; exit 1; }

CONFIG_DIR="$HOME/.ssh/config.d"
CONFIG_FILE="$HOME/.ssh/config"
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [[ ! -f "$CONFIG_FILE" ]]; then
  : > "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
fi
if ! grep -Fq 'Include ~/.ssh/config.d/*.conf' "$CONFIG_FILE"; then
  TEMP_CONFIG="$(mktemp)"
  printf '%s\n' 'Include ~/.ssh/config.d/*.conf' > "$TEMP_CONFIG"
  cat "$CONFIG_FILE" >> "$TEMP_CONFIG"
  cat "$TEMP_CONFIG" > "$CONFIG_FILE"
  rm -f "$TEMP_CONFIG"
  chmod 600 "$CONFIG_FILE"
fi
ssh-keygen -R star-labs >/dev/null 2>&1 || true
ssh-keygen -R "$PUBLIC_IP" >/dev/null 2>&1 || true
cat > "$CONFIG_DIR/star-labs-lab.conf" <<EOF
Host star-labs star-labs-lab
  HostName $PUBLIC_IP
  User ec2-user
  IdentityFile $KEY_FILE
  IdentitiesOnly yes
  StrictHostKeyChecking accept-new
EOF
chmod 600 "$CONFIG_DIR/star-labs-lab.conf"

printf 'Account: %s\nRegion: %s\nInstance: %s (t3.medium)\nSSH source: %s\nConnect: ssh star-labs\n' \
  "$ACCOUNT_ID" "$REGION" "$INSTANCE_ID" "$SSH_CIDR"
