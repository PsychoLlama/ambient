# Processes and Live Upgrade

Part of the [Ambient Language Reference](architecture.md).

> **Status: superseded as the upgrade design.** This process model was
> the experiment for giving live code upgrade a well-defined unit of
> state to hand off. Prototyping showed the reducer/mailbox boundary is
> the wrong unit, and the design has pivoted:
> **[live-upgrade.md](live-upgrade.md)** is now the live-upgrade design
> and wins wherever the two documents disagree. This document remains as
> the description of the implemented process runtime — parts of which
> (the deploy pass, runtime-owned state, the shared handle table) the
> new design generalizes and inherits — until the runtime catches up.
> Whether a mailbox/reducer library survives as a concurrency story is
> a separate, open decision.

Ambient's unit of long-lived state is the **process**: a named, isolated
reducer driven by a mailbox, in the spirit of Erlang's `gen_server`. The
process model exists for one reason above all others: **hot code
replacement needs a well-defined unit of state to hand off**, and a
mailbox/reducer boundary provides exactly that. Content addressing
supplies the other half — a deploy knows _precisely_ which processes'
code changed, because code identity is a hash.

## The model

A process is three things:

- a **name** (its durable identity, unique among live processes),
- a **reducer** `handler: (State, Msg) -> State`, and
- an **init** `init: () -> State` that produces the starting state.

The runtime owns the state. Between messages, state exists only as a
value held by the runtime; user code touches it exclusively inside a
reduction. That boundary is what makes upgrade, restart, and (future)
remote migration well-defined operations rather than heuristics.

Each process runs on its own OS thread with its own VM. All IO stays
blocking — a process blocked in `core::system::Network::accept!` blocks only itself.
Messages are ordinary values, delivered in send order, one reduction at
a time. There is no shared memory between processes.

```ambient
pub fn run(): () with core::system::Process {
  let counter = core::system::Process::spawn!("counter", () => 0, count_hits);
  core::system::Process::send!(counter, "hit");
}

fn count_hits(count: Number, msg: String): Number with core::system::Stdio {
  core::system::Stdio::out!("hits: ${to_string(count + 1)}");
  count + 1
}
```

### The Process ability

Declared in the platform bindings interface (`platform.ab`), performed
like any platform ability:

```ambient
ability Process {
  /// Spawn a named process; returns its pid. During a deploy pass this
  /// is a *declaration*: an existing live name is rebound instead.
  fn spawn<I, H>(name: String, init: I, handler: H): Number;
  /// Deliver a message. Never fails; sends to dead pids are dropped.
  fn send<M>(pid: Number, msg: M): ();
  /// Deliver a message to a named process (dropped if no such name).
  fn send_named<M>(name: String, msg: M): ();
  /// The calling process's pid (0 outside any process).
  fn self(): Number;
  /// The pid registered under a name, or 0 if none.
  fn whereis(name: String): Number;
  /// Stop the calling process after the current reduction.
  fn exit(): ();
}
```

`init` and `handler` are generic parameters rather than function types
because ability signatures cannot yet express effect-polymorphic
function parameters (`(S, M) -> S with E`); the runtime checks arity at
spawn time instead. Typed spawn is future work tied to effect variables
in ability declarations.

Handlers and inits may use the full platform ability set (Stdio, Log,
Time, Random, FileSystem, Network); their effects are granted by the
runtime that drives the reduction, not tracked through `spawn`'s
signature — the same host-boundary rule that governs `core::system::Execute::run`.

### Root vs dynamic processes

- **Root processes** are spawned during a _deploy pass_ (see below) —
  the program's entry function declaring its process tree. They are
  _reconciled_ on every deploy: rebound, kept, or stopped.
- **Dynamic processes** are spawned during ordinary reductions (e.g. one
  per accepted connection). They are pinned to their spawn-time code and
  run until they `exit!` or crash out; deploys never touch them. Old and
  new code coexist in the store and in the VMs, so this is safe by
  construction.

Spawning a name that is already live (outside a deploy pass) raises a
catchable `Exception`.

## Deploy passes: upgrade as reconciliation

The entry function (`pub fn run(): () with core::system::Process`) is not a program —
it is a **declaration of the desired process tree**. `ambient dev`
re-runs it against the running system every time the code changes, the
way a UI framework re-renders and reconciles:

1. Recompile the package. On error: report, keep the old generation
   running, untouched.
2. Run the new entry in **reconcile mode**. Every `spawn!` is matched
   against the live registry by name:
   - **Name exists, handler hash unchanged** → no-op. Content
     addressing makes "unchanged" exact: same function hash, same
     captured environment.
   - **Name exists, handler differs** → stage an upgrade. At the
     process's next reduction boundary it swaps to the new handler
     (and new init, used for future restarts) **keeping its state**.
     A process mid-reduction (or blocked in IO) finishes on old code
     first; the swap lands before the next message is processed.
   - **New name** → spawn it fresh.
3. Root processes the new entry did _not_ declare are stopped.
4. Every process VM (including pinned dynamic processes) gets the new
   generation's functions loaded alongside the old — additive and
   collision-free because functions are content-addressed. Old code
   keeps running; values referencing new code (e.g. closures inside
   messages) resolve.

State handoff is therefore _by default and by name_: a changed reducer
receives the state its predecessor left. There is no `code_change`
callback yet; the migration contract is:

- If the new reducer can consume the old state, it just does.
- If it can't — the reduction faults — supervision takes over: the
  process restarts with a fresh state from the _new_ init. Live data
  loss is bounded to that one process, and the failure is loud.

Because the entry re-runs on every deploy, it should stay
**declarative**: spawn processes, don't acquire resources. Bind
listeners inside a process's `init`, not in the entry.

## Supervision

Every process is supervised by the runtime:

- A faulted reduction (uncaught exception or engine fault) logs the
  error and **restarts the process**: fresh state from init, mailbox
  preserved, subsequent messages processed normally.
- Five consecutive faults (without an intervening successful reduction)
  park the process as dead; the next deploy pass may re-spawn it.
- A faulted init kills the process outright — its environment is wrong,
  and retrying init in a loop would flap.

This is deliberately the smallest useful supervisor. Supervision
_trees_ (per-child restart strategies, escalation) are future work and
should be expressible in-language once process linking exists.

## Interaction with blocking IO

Reductions may block (accept a connection, read a socket). The rules
that fall out:

- A blocked process delays _its own_ upgrade until the reduction
  completes; everyone else proceeds. An acceptor loop structured as
  "reduce per accepted connection" (see `examples/live_server`)
  upgrades on the next connection.
- The runtime shares one network handle table across all processes, so
  a listener accepted in one process can be served by another. Blocking
  operations hold only per-connection locks, never the table lock.

## Relation to Execute (remote upgrade)

Nothing here is dev-server-specific. A deploy pass is: _a code
generation (pack of content-addressed objects) plus one run of an entry
function in reconcile mode_. `ambient dev` produces generations from
the local file watcher; a future `Execute`-driven path can receive the
same pack over the wire and run the same reconciliation — live upgrade
of a remote service with the exact mechanics tested locally. The
process runtime is engine-adjacent (in `ambient-platform`), takes code
generations as plain data, and does not know where they came from.

## Current limits

- Messages and state are in-memory values; nothing is persisted.
- No selective receive, no receive timeouts, no process linking or
  monitors; `send` is fire-and-forget.
- No typed `spawn` (see above) — arity is checked at runtime.
- No supervision trees; the flat runtime supervisor restarts in place.
- Stopping a process blocked in IO takes effect at its next reduction
  boundary.
