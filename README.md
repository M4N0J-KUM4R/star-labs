# star-labs

Incremental Canvas LMS migration from Rails to Rust.

## Temporary AWS lab bootstrap

This repository includes a repeatable setup for the restricted 3-hour lab. It
uses the lab's existing CLI credentials and `LabInstanceProfile`; it does not
create IAM users, roles, or policies. The launcher creates one Amazon Linux
2023 EC2 instance (`t3.medium`), a 100 GB encrypted `gp3` root volume, and an
SSH-only security group restricted to the operator's current IPv4 `/32`.
The Dahl proxy port is not opened in the security group; it runs on loopback
for Hermes on the same server.

Start the lab in AWS Academy, copy its temporary CLI credentials using the
lab's **AWS Details → AWS CLI** instructions, and run:

```sh
AWS_REGION=us-west-2 ./scripts/lab-up.sh
ssh star-labs
```

The instance bootstrap installs the official Hermes CLI and Rust toolchain.
Provision private PostgreSQL, a private encrypted S3 bucket, and a short-lived
CloudWatch log group with:

```sh
AWS_REGION=us-west-2 ./scripts/lab-data-up.sh
```

The database is `db.t3.micro`, 20 GB gp2, non-public, single-AZ, and accepts
PostgreSQL only from the EC2 security group. A random DB password is saved in
`~/.config/star-labs/db-password` with mode 600 and is never written to GitHub.
The S3 bucket blocks public access and expires temporary objects after three
days.

After the instance is ready, deploy the verified Rust source and local config:

```sh
STAR_LMS_SOURCE_DIR=/path/to/clean/rust-migration-checkout \
DAHL_PROXY_DIR="$HOME/projects/dahl-proxy" \
HERMES_HOME="$HOME/.hermes" \
./scripts/lab-deploy.sh
```

The deploy script copies only committed Rust source and transfers Hermes/Dahl
configuration over SSH with restrictive file modes. It refuses a dirty source
checkout and never commits provider credentials. The old local checkout is not
assumed to be canonical; provide a clean, verified source directory.

At the end of a lab session, remove its resources with:

```sh
AWS_REGION=us-west-2 ./scripts/lab-down.sh
```

The cleanup script empties and deletes the data stack before removing the
tagged EC2 resources and the local temporary DB password.

Keep temporary AWS credentials out of source files, shell history, and GitHub
Actions. They should only be available in the current CLI environment/profile.
