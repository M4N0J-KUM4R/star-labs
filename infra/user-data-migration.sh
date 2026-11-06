#!/bin/bash
set -euo pipefail
exec > >(tee -a /var/log/star-labs-migration-bootstrap.log) 2>&1

dnf install -y git gcc make openssl-devel pkgconfig tar gzip rsync
runuser -l ec2-user -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'
install -d -o ec2-user -g ec2-user -m 0750 /home/ec2-user/work
if ! swapon --show=NAME --noheadings | grep -qx /swapfile; then
  fallocate -l 2G /swapfile
  chmod 600 /swapfile
  mkswap /swapfile
  swapon /swapfile
  printf '%s\n' '/swapfile swap swap defaults 0 0' >> /etc/fstab
fi
touch /var/log/star-labs-migration-bootstrap.complete
