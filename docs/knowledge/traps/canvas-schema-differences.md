---
tags: [trap, canvas, ruby, schema]
---
# Trap: this Canvas version's schema is not the schema you remember

Columns and models that **do not exist** here — assuming they do produces
`ActiveModel::UnknownAttributeError` or `PG::UndefinedColumn`:

| you expect | actually |
|---|---|
| `accounts.account_type` | no such column; an account is a root when `root_account_id == 0` |
| `accounts.settings_method` | no such column; `settings` is a JSON column |
| `users.login_id` | no such column — the login lives on `communication_channels` |
| `users.email` | no such column — also on `communication_channels` |
| `User.find_by_login` | no such method; find the user by `name` or via its channel |
| `GroupMember` | it is `GroupMembership` |
| `Account#root_account?` | `root_account_id.zero()` — see [[canvas-root-account]] |

**How to check instead of guessing.** Read the live schema, never your memory of
Canvas:

```sh
docker exec canvas-boot-postgres-1 bash -lc \
  "PGPASSWORD=canvas_sekret psql -U canvas -d canvas_production -tAc \
   \"select column_name from information_schema.columns where table_name='accounts'\""
```

Or, from inside the container, ask the model:
`bundle exec rails runner 'puts Account.column_names.join(", ")'`.

**Related.** [[fact-schema-truth]].
