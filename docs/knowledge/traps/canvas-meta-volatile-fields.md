---
tags: [trap, parity, canvas, harness]
---
# Trap: `x-canvas-meta` carries process diagnostics that no other process can reproduce

**The header.** Canvas sends a semicolon-separated diagnostic string on every
API response:

```
at=10000000000007;dk=10000000000001;o=accounts;n=index;b=0;m=0;u=0.00;y=0.00;d=0.00;rlr=600.0;
```

**Which fields are the contract.** `at` (the access token), `dk` (the
developer key), `o` (controller), `n` (action), `t` (type), `i` (id) and
`rlr` (rate limit remaining). These must match exactly.

**Which are not.** `b` (bytes), `m` (memory), `u` (cpu seconds), `y` (memory
MB), `d` (db seconds) — the *Canvas server's* numbers, on the machine that
answered. A Rust port can never reproduce another process's memory bytes or
its query timing, and the recorder wildcarded them inline (`b=[..]`), which
the text differ does **not** treat as a pattern.

**The fix, and the trap inside it.** `migration/tools/parity/meta_tool.py`:

```sh
python3 tools/parity/meta_tool.py normalize rust/app-cases/http   # side file
python3 tools/parity/meta_tool.py check    rust/app-cases/http --url ...
python3 tools/parity/meta_tool.py restore  rust/app-cases/http
```

`normalize` moves the recorded meta into `<case>.meta` and leaves the compared
header as the **whole-value** wildcard `[..]` — which the differ *does*
honour. `check` then compares only the stable fields, field by field.

**Do not skip this.** Whole-value wildcards work; inline ones do not. 327
cases failed on nothing but an inline wildcard before this.

**Related.** [[canvas-response-headers]], [[canvas-access-token-auth-model]],
[[corpus-token-placeholder]].
