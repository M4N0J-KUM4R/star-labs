#!/usr/bin/env bash
set -euo pipefail

REGION="${AWS_REGION:-us-west-2}"
BASE_NAME="star-labs-lab"

BASE_ID="$(aws ec2 describe-instances --region "$REGION" \
  --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
    Name=tag:Name,Values="$BASE_NAME" Name=instance-state-name,Values=running \
  --query 'Reservations[0].Instances[0].InstanceId' --output text)"
[[ "$BASE_ID" != None && -n "$BASE_ID" ]] || { echo 'Base star-labs-lab instance is not running.' >&2; exit 1; }

read -r AMI_ID SUBNET_ID SG_ID KEY_NAME < <(aws ec2 describe-instances --region "$REGION" \
  --instance-ids "$BASE_ID" \
  --query 'Reservations[0].Instances[0].[ImageId,SubnetId,SecurityGroups[0].GroupId,KeyName]' \
  --output text)

for SPEC in 'star-labs-agents:agents-user-data.sh' 'star-labs-migration:migration-user-data.sh'; do
  INSTANCE_NAME="${SPEC%%:*}"
  USER_DATA_FILE="${SPEC#*:}"
  INSTANCE_ID="$(aws ec2 describe-instances --region "$REGION" \
    --filters Name=tag:Project,Values=star-labs Name=tag:Purpose,Values=learner-lab \
      "Name=tag:Name,Values=$INSTANCE_NAME" Name=instance-state-name,Values=pending,running \
    --query 'Reservations[0].Instances[0].InstanceId' --output text)"

  if [[ "$INSTANCE_ID" == None || -z "$INSTANCE_ID" ]]; then
    RUNNING_COUNT="$(aws ec2 describe-instances --region "$REGION" \
      --filters Name=instance-state-name,Values=pending,running \
      --query 'Reservations[].Instances[].InstanceId' --output text | awk '{print NF}')"
    RUNNING_COUNT="${RUNNING_COUNT:-0}"
    (( RUNNING_COUNT < 9 )) || { echo 'AWS lab limit of nine running instances reached.' >&2; exit 1; }
    INSTANCE_ID="$(aws ec2 run-instances --region "$REGION" \
      --image-id "$AMI_ID" --instance-type t3.medium --key-name "$KEY_NAME" \
      --iam-instance-profile Name=LabInstanceProfile --subnet-id "$SUBNET_ID" \
      --security-group-ids "$SG_ID" --count 1 --associate-public-ip-address \
      --metadata-options HttpEndpoint=enabled,HttpTokens=required \
      --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":100,"VolumeType":"gp3","Encrypted":true,"DeleteOnTermination":true}}]' \
      --user-data "file://$HOME/work/bootstrap/$USER_DATA_FILE" \
      --tag-specifications \
        "ResourceType=instance,Tags=[{Key=Name,Value=$INSTANCE_NAME},{Key=Project,Value=star-labs},{Key=Purpose,Value=learner-lab},{Key=Role,Value=${INSTANCE_NAME##*-}}]" \
        "ResourceType=volume,Tags=[{Key=Name,Value=$INSTANCE_NAME-root},{Key=Project,Value=star-labs},{Key=Purpose,Value=learner-lab}]" \
      --query 'Instances[0].InstanceId' --output text)"
  else
    INSTANCE_TYPE="$(aws ec2 describe-instances --region "$REGION" --instance-ids "$INSTANCE_ID" \
      --query 'Reservations[0].Instances[0].InstanceType' --output text)"
    [[ "$INSTANCE_TYPE" == t3.medium ]] || { echo "$INSTANCE_NAME has unexpected type $INSTANCE_TYPE." >&2; exit 1; }
  fi

  aws ec2 wait instance-running --region "$REGION" --instance-ids "$INSTANCE_ID"
  PUBLIC_IP="$(aws ec2 describe-instances --region "$REGION" --instance-ids "$INSTANCE_ID" \
    --query 'Reservations[0].Instances[0].PublicIpAddress' --output text)"
  printf '%s\t%s\t%s\n' "$INSTANCE_NAME" "$INSTANCE_ID" "$PUBLIC_IP"
done
