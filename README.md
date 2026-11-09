# star-labs

Incremental Canvas LMS migration from Rails to Rust.

## Switching to a fresh lab account

The migration source and infrastructure templates are versioned here; runtime AWS
credentials, DB passwords, the live database, production data, and provider keys
are not. The Rust snapshot is under `migration/`; the lab stack template is
`infra/lab-compute-stack.yaml` plus `infra/lab-data-stack.yaml`.

1. Start the new AWS Academy lab and configure its temporary CLI credentials
   using the lab portal instructions. Do not paste credentials into this repo.
2. Verify the active identity with `aws sts get-caller-identity`, then clone this
   repo and enter it:

   ```sh
   git clone https://github.com/M4N0J-KUM4R/star-labs.git
   cd star-labs
   ```

3. Create a fresh base instance, its workers, and data services in the lab region:

   ```sh
   AWS_REGION=us-west-2 ./scripts/lab-compute-up.sh
   ./scripts/lab-workers-up.sh
   AWS_REGION=us-west-2 ./scripts/lab-data-up.sh
   ```

4. After checking out the Rust source and verifying it, deploy it with the
   locally held Hermes and Dahl directories (these may contain credentials and
   must stay out of Git):

   ```sh
   STAR_LMS_SOURCE_DIR="$PWD/migration" \
   DAHL_PROXY_DIR="$HOME/projects/dahl-proxy" \
   HERMES_HOME="$HOME/.hermes" \
   ./scripts/lab-deploy.sh
   ```

5. Use `ssh star-lms` (or `ssh star-labs`), `ssh star-labs-agents`, and
   `ssh star-labs-migration` to connect. Verify each endpoint and CI before
   loading any real data. The current RDS contains only synthetic test rows.

The compute template installs Hermes and Rust and creates the `t3.medium` host.
CloudFormation cannot read local files from your workstation. After launch,
`lab-deploy.sh` sends Hermes configuration, Dahl account/config files, and the
private Dahl source over SSH with restrictive permissions; it builds and starts
Dahl bound to loopback. Those files and session credentials are never put in
template parameters, user data, or S3. The previous lab’s EC2, RDS, and S3 data
do not transfer to a new account. This repo recreates infrastructure and source;
it does not contain a production data backup. The current GitHub Actions workflow
validates Rust code against a fresh PostgreSQL service.

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
AWS_REGION=us-west-2 ./scripts/lab-compute-up.sh
ssh star-labs
```

The CloudFormation compute stack installs the official Hermes CLI and Rust
toolchain, creates a 100 GB encrypted `gp3` root volume, and opens only SSH from
the operator's `/32`. Dahl's port is not opened in the security group.

To add one dedicated Hermes worker and one Rust build/migration worker (both
within the lab's `t3.medium` limit), run:

```sh
./scripts/lab-workers-up.sh
ssh star-labs-agents
ssh star-labs-migration
```

The launcher uses the base instance's existing lab role, key pair, subnet, and
SSH-only security group. It reuses already-running named workers and checks the
nine-instance lab limit before each launch.

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
configuration over SSH with restrictive file modes. It supports the `migration/`
subdirectory in this repository. It refuses a dirty checkout and never commits
provider credentials.

When a migration source is deployed and the data stack env file
(`~/.config/star-labs/data.env`) exists on the host, the same script also
builds the star-lms API in release mode (`cargo build --release --locked`),
installs the binary as `/usr/local/bin/star-lms`, provisions
`/etc/star-lms/star-lms.env` (mode 600, `DATABASE_URL` from the data stack
plus a generated `STAR_JWT_SECRET`), and enables the `infra/star-lms.service`
systemd unit (restart-on-failure, hardened). The JWT secret is generated on
the host and never stored in this repository.

At the end of a lab session, remove its resources with:

```sh
AWS_REGION=us-west-2 ./scripts/lab-down.sh
```

The cleanup script empties and deletes the data stack before removing the
tagged EC2 resources and the local temporary DB password.

Keep temporary AWS credentials out of source files, shell history, and GitHub
Actions. They should only be available in the current CLI environment/profile.
