---
tags: [trap, canvas, ruby, bundler]
---
# Trap: `bundler-multilock` is a bundler PLUGIN, not a gem

**Symptom.** `bundle exec` dies with
`[!] There was an error parsing 'Gemfile': bundler-multilock plugin is not installed.`

**Cause.** Canvas' `Gemfile` declares:

```ruby
plugin "bundler-multilock", "1.4.0", path: "#{__dir__}/vendor/gems/bundler-multilock"
```

`gem install bundler-multilock` does **not** satisfy it — the plugin must be
installed into bundler's plugin directory:

```sh
bundle plugin install bundler-multilock
```

**Watch out.** Installing it also pulls in bundler 4.x, which breaks the pinned
`BUNDLED WITH 2.6.7` in `Gemfile.lock`. Undo it with
`gem uninstall bundler -v 4.1.1 -x --force` and check `bundle --version` is 2.6.x.

**Related.** [[canvas-special-accounts-production]].
