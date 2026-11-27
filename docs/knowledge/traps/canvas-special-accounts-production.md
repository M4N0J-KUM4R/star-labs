---
tags: [trap, canvas, ruby]
---
# Trap: Canvas' special accounts are not auto-created in production

**Symptom.** Every request answers 500 with
`NoMethodError: undefined method 'id' for nil` at
`lib/host_url.rb:87` (`HostUrl.file_host_with_shard`).

**Cause.** `Account.default` is a *special account* resolved through the
`default_account_id` **Setting**. Canvas only auto-creates special accounts when
`!Rails.env.production?` — and the oracle runs in production mode, because
`config/database.yml` in this checkout only has a `production:` section
(`host: postgres`, `database: canvas_production`, `password: canvas_sekret`).

**Fix.** Force-create them and point the settings at them:

```ruby
default_account = Account.default(force_create: true)
site_admin      = Account.site_admin(force_create: true)
Setting.set("default_account_id", default_account.id)
Setting.set("site_admin_account_id", site_admin.id)
```

`Account.default` is also what `HostUrl.file_host_with_shard` resolves, so
without it every single request 500s — it looks like a broken app, not a
missing account. See [[canvas-root-account]].
