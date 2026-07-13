# live_site — a live-upgradable web site

A tiny HTTP site built on the post-process upgrade model: **tasks +
cells + generations** (see `ref/live-upgrade.md`). It exists to be
edited while it runs.

## The model in one paragraph

Content addressing means a hash's behavior can never change, so an
upgrade is only ever a **name → hash rebinding**. A deploy is: compile a
new generation (content-addressed objects + name bindings), load it
additively into the running system, swap the name table atomically, and
re-run the entry as a reconciliation pass. Running code picks up new
behavior exactly where it re-enters through a **late-bound point**:
`Live::latest!(f)`, a task's per-pass body resolution, or the entry
itself. Everything below such a point is fresh on next entry;
everything above one is pinned — correctly, by construction. State
survives because it never belongs to a generation: it lives in
**runtime-owned named cells**, adopted (or migrated) by each deploy
rather than handed off.

## Running it

```
ambient dev examples/live_site
```

Then browse (or curl) `http://127.0.0.1:7878/` — and, while it serves:

1. **Edit `GREETING` in `handlers.ab`** and save. The next request
   serves the new text, and the hit counter keeps counting. No task
   restarted; the acceptor never stopped accepting.
2. **Change the `Stats` shape in `state.ab`** (demote `Stats` to the
   next `*V2` relic — keeping its uuid — declare the new `Stats`, and
   extend `migrate`; `handlers.ab`'s update lambda must construct the
   new shape). The deploy migrates the live cell in place; the hit
   count survives.
3. **Delete the `Task::ensure!("ticker", …)` line in `main.ab`.** The
   reconciler stops declaring the task, so the runtime drains it:
   `Drain::requested!` unwinds it out of its next `Time::wait!`, its
   handler prints a goodbye, done. Never mid-tick, never a kill.
4. **Break something.** The deploy fails to compile or type-check; the
   previous generation keeps running, untouched.

## How the pieces upgrade

| Thing                                | Fresh when…                                                                    |
| ------------------------------------ | ------------------------------------------------------------------------------ |
| Entry (`run`)                        | every deploy (it _is_ the reconciliation pass)                                 |
| A task's body                        | the next pass — the runtime re-resolves the body's deployed name per iteration |
| Anything reached via `Live::latest!` | the next call through the latest point                                         |
| A `const`                            | whenever a function reading it is fresh (the new hash embeds the new const)    |
| Cell contents                        | never, by design — only a migration changes live state                         |
| Code above every late-bound point    | never                                                                          |

One adjustment against the original design sketch: there is no `Http`
ability, and the language has no tail calls yet, so the loop the ref
doc spells in-language (`Live::latest!(accept_loop)()`) lives in the
task runtime instead — a task body is **one bounded pass**, re-invoked
by the runtime with the name resolution applied between passes. The
HTTP framing is hand-rolled over `Tcp::receive_raw`/`send_raw`;
the upgrade mechanics are the point, not the protocol.

## Files

- `main.ab` — the entry as a declaration: the stats cell, the listener
  handle, the named tasks. Safe to re-run on every deploy because
  everything in it adopts.
- `router.ab` — the late-bound entry into request handling.
- `handlers.ab` — the hot part of the tree; edit these live.
- `state.ab` — the versioned-state migration contract, spelled out.
- `ticker.ab` — a background task: per-pass freshness and graceful
  drain.
