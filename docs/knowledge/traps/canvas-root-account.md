---
tags: [trap, canvas, ruby]
---
# Trap: Canvas' `root_account?` means `root_account_id.zero()`, not `root_account_id == id`

**Symptom.** Seeding Canvas fails with
`NoMethodError: undefined method 'id' for nil` at
`app/models/account.rb:1310` (`Account#account_chain`), for any account whose
`root_account_id` points at id 0.

**The rule.** In this Canvas version, `Account#root_account?` is:

```ruby
def root_account?
  if root_account_id
    local_root_account_id.zero?     # <- zero, NOT == id
  ...
```

So the ROOT account is the one whose `root_account_id` is **0**. Id 0 is a
*deleted* "Dummy Root Account" created by the `LoadInitialData` migration.

**Two ways to get it wrong.**
1. Pointing accounts at a self-referential `root_account_id = own id` — that is
   not a root by Canvas' definition, and `account_chain` blows up.
2. Leaving accounts pointing at the deleted dummy (id 0) — also breaks the chain.

**Fix.** Give every live account `root_account_id = 0` and make sure at least one
account exists. See `canvas-boot/seed.rb`.

**Related.** [[canvas-special-accounts-production]].
