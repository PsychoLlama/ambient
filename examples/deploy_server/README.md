# deploy_server — remote live upgrade

live_site's upgrade mechanics with the dev loop removed: the running
program is upgraded **over the wire**, by shipping a generation pack to
a port the program itself listens on. One mechanism, third frontend —
the same deploy core serves `ambient dev`'s watcher, the REPL's turns,
and this example's `Deploy::apply!`.

## The tour

Start the service — plain `run`, no watcher:

```bash
ambient run examples/deploy_server
```

It serves framed requests on `127.0.0.1:7910` and listens for
generation packs on `127.0.0.1:7911` (override with `SERVICE_PORT` /
`DEPLOY_PORT`). Now edit `handlers.ab` — change the greeting — and
build the next generation as a shippable artifact:

```bash
ambient build examples/deploy_server -o v2.ambient
```

Ship it with the paired client, which holds one conversation across
the upgrade:

```bash
DEPLOY_PACK=v2.ambient ambient run examples/deploy_client
```

The client's first request is answered by generation one; then it
sends the pack and prints the server's deploy report (the exact name
diff — which items rebound, what stayed); then its second request — on
the **same connection** — is answered by generation two, with the hit
counter carried across, because the counter is a runtime-owned cell
and never belonged to either generation.

What makes the mid-conversation landing work is in `service.ab`: the
whole conversation runs inside one task pass, so the task runtime's
per-pass re-resolution never gets a chance to help — each request is
dispatched through `Live::latest!(handlers::respond)`, the
author-placed late-bound point, and the deploy's rebinding lands on
the very next request.

A rejected deploy (malformed pack, failed validation, a pending
migration that doesn't match the live cell) is the `Err` arm of
`Deploy::apply!` — the sender gets the reason, the running program is
untouched and keeps serving.

## Trust

**Whoever can reach the deploy port owns this runtime.** A generation
pack is arbitrary code, applied with the full capabilities of the
host. Hashes are recomputed from the received bytes — a tampered pack
cannot smuggle code under a false hash — but there is no
authentication, authorization, or transport security here at all.
This is the hobby-deployment posture of `ref/remote-execution.md`:
bind the deploy listener to loopback, tunnel to it, or wrap the
transport yourself.
