#!/usr/bin/env bash
set -euo pipefail

PROFILE="${AWS_PROFILE:-}"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-$(aws configure get region --profile "${PROFILE:-default}" 2>/dev/null || true)}}"
REGION="${REGION:-us-west-2}"
STACK_NAME="${STAR_LABS_STACK_NAME:-star-labs-data}"
aws_lab() {
  if [[ -n "$PROFILE" ]]; then
    command aws --profile "$PROFILE" "$@"
  else
    command aws "$@"
  fi
}

if aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" >/dev/null 2>&1; then
  BUCKET_NAME="$(aws_lab cloudformation describe-stacks --region "$REGION" --stack-name "$STACK_NAME" \
    --query 'Stacks[0].Outputs[?OutputKey==`BucketName`].OutputValue|[0]' --output text)"
  if [[ -n "$BUCKET_NAME" && "$BUCKET_NAME" != None ]]; then
    aws_lab s3 rm "s3://$BUCKET_NAME" --recursive --region "$REGION"
  fi
  aws_lab cloudformation delete-stack --region "$REGION" --stack-name "$STACK_NAME"
  aws_lab cloudformation wait stack-delete-complete --region "$REGION" --stack-name "$STACK_NAME"
fi

if ssh -o BatchMode=yes -o ConnectTimeout=5 star-labs true >/dev/null 2>&1; then
  ssh star-labs 'rm -f "$HOME/.config/star-labs/data.env"'
fi

INSTANCE_IDS="$(aws_lab ec2 describe-instances --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
    Name=instance-state-name,Values=pending,running,stopping,stopped \
  --query 'Reservations[].Instances[].InstanceId' --output text)"
if [[ -n "$INSTANCE_IDS" && "$INSTANCE_IDS" != None ]]; then
  read -r -a INSTANCE_ARRAY <<< "$INSTANCE_IDS"
  aws_lab ec2 terminate-instances --region "$REGION" --instance-ids "${INSTANCE_ARRAY[@]}" >/dev/null
  for INSTANCE_ID in "${INSTANCE_ARRAY[@]}"; do
    aws_lab ec2 wait instance-terminated --region "$REGION" --instance-ids "$INSTANCE_ID"
  done
fi

GROUP_IDS="$(aws_lab ec2 describe-security-groups --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
  --query 'SecurityGroups[].GroupId' --output text)"
if [[ -n "$GROUP_IDS" && "$GROUP_IDS" != None ]]; then
  read -r -a GROUP_ARRAY <<< "$GROUP_IDS"
  for GROUP_ID in "${GROUP_ARRAY[@]}"; do
    aws_lab ec2 delete-security-group --region "$REGION" --group-id "$GROUP_ID"
  done
fi

if aws_lab ec2 describe-key-pairs --region "$REGION" --key-names star-labs-lab >/dev/null 2>&1; then
  aws_lab ec2 delete-key-pair --region "$REGION" --key-name star-labs-lab
fi
rm -f "$HOME/.ssh/config.d/star-labs-lab.conf" "$HOME/.ssh/star-labs-lab.pem"
rm -f "$HOME/.config/star-labs/db-password" "$HOME/.config/star-labs/data.env"
ssh-keygen -R star-labs >/dev/null 2>&1 || true
printf 'Removed tagged learner lab resources in %s.\n' "$REGION"
