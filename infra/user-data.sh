#!/bin/bash
set -euo pipefail
exec > >(tee -a /var/log/star-labs-bootstrap.log) 2>&1

# Amazon Linux 2023 ships curl-minimal; installing the full curl package causes
# a conflict, so keep the image's preinstalled curl implementation.
dnf install -y git gcc make openssl-devel pkgconfig tar gzip rsync

# Rust is needed to build the private Dahl proxy source copied after launch.
runuser -l ec2-user -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'

# Install the official Hermes CLI into ec2-user's home; provider configuration
# is copied over SSH later and is never stored in this repository or user data.
runuser -l ec2-user -c 'curl --proto "=https" --tlsv1.2 -fsSL https://hermes-agent.nousresearch.com/install.sh | bash'

install -d -o ec2-user -g ec2-user -m 0750 /home/ec2-user/work
if ! swapon --show=NAME --noheadings | grep -qx /swapfile; then
  fallocate -l 2G /swapfile
  chmod 600 /swapfile
  mkswap /swapfile
  swapon /swapfile
  printf '%s\n' '/swapfile swap swap defaults 0 0' >> /etc/fstab
fi
touch /var/log/star-labs-bootstrap.complete
