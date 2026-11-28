# Production cutover

The port replaces Canvas' API one route at a time. The service is deployed
next to the running Canvas instance, answers the routes it owns, and sends
the rest to Canvas through the front door
(`crates/star-proxy`). Nothing a user sees changes on the first day; every
route that the recorded corpus proves moves onto the port, one family at a
time.

## What the service is

One binary: `star-http`. It owns the API, the GraphQL API, the server-rendered
UI shell and the front door.

| the request | who answers it |
|---|---|
| a route the port owns | the port |
| a route the port does not own | Canvas, through the front door |
| a path the port does not own and no origin is set | 404 |

The port applies its own migrations on start. Every migration is additive
(`ALTER TABLE ... ADD COLUMN`), so the port can run against a Canvas database
that already has the data: nothing is dropped, renumbered or rewritten.

## The environment

| variable | what it is |
|---|---|
| `DATABASE_URL` | the Canvas PostgreSQL database |
| `STAR_JWT_SECRET` | the secret the port keys the access-token hash with; the same value the deployment uses everywhere |
| `STAR_PORT` / `PORT` | the port the service listens on (8099 by default) |
| `STAR_CANVAS_ORIGIN` | the Canvas instance the front door delegates to; **unset means no delegation** |
| `STAR_CANVAS_DOMAIN` | the domain Canvas names in its `content-security-policy` header; the recorded answers carry it |

`STAR_CANVAS_ORIGIN` is the rollback: unset it and the port answers only the
routes it owns, 404 for the rest. Set it and the deployment is complete
while the port catches up.

## The steps

1. **Deploy with the front door on.** `STAR_CANVAS_ORIGIN` points at the
   running Canvas. The port serves the routes it owns and proxies the rest;
   the traffic is unchanged and the port is now the edge.
2. **Watch the corpus.** `migration/rust/app-cases/` holds Canvas' recorded
   answers (status, headers, body) for 460 cases. The replay in CI
   (`rust-anything app replay`) diffs the port's answers against them and
   fails when a case that passed regresses.
3. **Flip a family.** A route moves off the front door when its family
   passes the corpus. The scoreboard (`tools/parity/parity.py report`) says
   how many of Canvas' routes the port owns.
4. **Roll back** by unsetting `STAR_CANVAS_ORIGIN` and restarting. No
   redeploy, no data change.

## What the corpus does not cover

The corpus is 460 cases; Canvas mounts 1292 API routes. A route with no
recorded answer is *mounted*, not *proven*. The scoreboard separates the
two: "mounted" is the route table, "proven" is the replay.

The UI is the other gap: the port ships a server-rendered shell, not
Canvas' React frontend (5,459 TSX files). The front door delegates the
browser's requests to Canvas until that surface is ported, so the UI is
never the reason a cutover stalls.

## The oracle

`canvas-boot/` runs a real Canvas (Rails 8, Ruby 3.4, PostgreSQL 16) with
the schema and the seed the corpus was recorded against. It is how a
disagreement is settled: the recorded answer wins, and when the port and
the tests disagree with it, the oracle is asked directly.
