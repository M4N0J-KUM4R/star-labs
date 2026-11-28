---
tags: [trap, permissions, feature-flags]
---
# Trap: 45 permissions are gated on an account feature flag the port never grants

**Status: KNOWN DIVERGENCE** — and the reason the two bulk-user routes
answer 403.

Canvas' permission registry gates 45 permissions on an account feature:
`account_allows: ->(a) { a.root_account.feature_enabled?(:some_flag) }`
(`config/initializers/permissions_registry.rb`). The flags are off by
default, so **the permission is denied until the account turns it on** —
`horizon_bulk_api_permission` is the one behind `manage_users_in_bulk`,
which is what `DELETE /accounts/:id/users` and
`PUT /accounts/:id/users/bulk_update` ask for
(`app/controllers/accounts_controller.rb:1885` and `:1920`).

The port's registry carries the same 45 permissions with
`account_allows: AccountAllows::Required`, and the permission layer
answers that flag with `false` — the port does not carry the flags, so
they are denied rather than allowed
(`crates/star-permissions/src/rules.rs`, `account_allows()`).

**What that means.** For those 45 permissions the port's answer is
always "no", while Canvas says "yes" for an account that has enabled the
flag. For a flag that is off by default — `horizon_bulk_api_permission`
among them — the port agrees with Canvas (the recorded corpus answers 403
for both bulk routes ✓). For a flag an account has turned on, the port
would refuse what Canvas allows.

**The fix.** The registry has to carry the flag name each permission is
gated on (Canvas names 23), and the check has to read the root account's
`feature_flags` rows — the port has the table. Until then, the port's
behaviour for these permissions is "the account has not enabled the
feature", which is right for the default and wrong for an account that
has.

**How to see the list.** `crates/star-permissions/src/registry.rs` — the
permissions with `account_allows: AccountAllows::Required`.

**The trap inside the trap.** A permission that is always denied looks
exactly like a permission that is correctly refused. The two are only
distinguishable by reading Canvas' registry for the flag, not by looking
at the port's answer.
