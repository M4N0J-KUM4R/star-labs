# Star LMS Rust migration snapshot

This directory is a source snapshot of the Rust migration workspace recovered from the temporary migration EC2. It deliberately excludes the Rails source, local Git history, production data, AWS credentials, and `.env` files.

## Build and test

Install stable Rust, then run:

```sh
cargo test --workspace
```

Live API parity tests are ignored by default. To run them, point their documented `STAR_PARITY_URL` (or crate-specific settings) to a local Star LMS service and provide `STAR_ADMIN_EMAIL` and `STAR_ADMIN_PASSWORD` through the environment.

DB-backed suites require a PostgreSQL `DATABASE_URL` and apply the `star-db` migrations as part of their setup where configured. The lab RDS endpoint and credentials are intentionally not stored here.

## Status

This is an in-progress migration snapshot, not a production-ready replacement for Canvas. Only the auth, modules, and enrollments database suites have been exercised against the new lab RDS so far; all workspace test targets have been compile-checked. Feature coverage, data migration, load testing, and production cutover remain incomplete.
