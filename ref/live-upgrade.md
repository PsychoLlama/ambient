# Live Upgrade

Part of the [Ambient Language Reference](architecture.md).

> **Status: in progress.** This document supersedes the process model
> ([processes.md](processes.md)) as the live-upgrade design; where the
> two disagree about upgrades, this document wins. It is written
> declaratively, as the target. Implemented so far: generations and the
> deploy core (`crates/ambient-platform/src/deploy.rs`), the `Live`
> ability with the same-signature rebinding rule, `State` cells with
> adopt semantics (`crates/ambient-platform/src/state.rs`; the cell
> table is owned by the deploy runtime and shared by every VM it
> builds), the migration contract (`init_versioned` with
> compiler-threaded fingerprints — see "Migration"), drain with
> interruptible performs (`crates/ambient-platform/src/drain.rs`),
> tasks (`crates/ambient-platform/src/task.rs`, reconciled beside the
> process registry by one deploy pass — `examples/live_site` is the
> working demonstration), retirement with the deploy diagnostics
> (`crates/ambient-platform/src/retire.rs`: the generation ledger, the
> trace, the two warnings, and the dev loop's store gc), and the REPL
> as a deploy frontend (`crates/ambient-cli/src/repl/`: every turn is an
> _incremental_ deploy — definitions validate-and-swap so a program
> driven from the prompt live-upgrades, and nothing is stopped for
> being absent from a turn), and remote deploy (the `Deploy` ability:
> a generation pack received over any transport is applied through the
> same core — packs carry signatures and migration obligations since
> pack v2, and `examples/deploy_server` is the working demonstration;
> see [remote-execution.md](remote-execution.md) for the trust
> posture). The process model remains in the tree as a
> concurrency experiment whose own future is decided separately (see
> "Relation to the process model").

## The model

Content addressing fixes what "upgrade" can possibly mean. A hash's
behavior can never change, so a live upgrade never edits running code —
it **rebinds names to new hashes**, and running code picks the
rebinding up wherever it re-enters through a name. Ambient calls such a
place a **late-bound point**. There are exactly three:

1. **The entry function.** A deploy re-runs it as a reconciliation
   pass, so the entry and everything it evaluates is fresh on every
   deploy.
2. **A `Live::latest!` perform** — a first-class, author-placed
   late-bound reference (see "The Live ability").
3. **A task name** — the runtime's registry of named long-running
   loops (see "Tasks").

Everything _below_ a late-bound point is upgraded automatically the
next time control enters through it: the resolved function's own hash
pins every internal call, so the subtree it reaches is new code,
consistently, all the way down. Everything _above_ every late-bound
point — in-flight frames, closures captured in values — is pinned to
the generation that created it, correctly and by construction. Old and
new generations coexist in one store and one set of VM tables (function
loading is additive and hash-keyed, so coexistence is collision-free);
neither generation can ever observe a torn mixture of the two, because
the only way to reach code is a hash (pinned) or a name (resolved
against one atomic table).

Everything else in this document is machinery in service of that
paragraph: **generations** are what a deploy ships, **cells** are why
state survives it, **drain** is how blocked code yields to it, and
**retirement** is how it is known to have finished.

## Generations

A **generation** is one deployable snapshot of a program:

- a pack of canonical content-addressed objects (the existing pack
  format — functions, groups, values, natives), plus
- **name bindings**: `Fqn → (hash, canonical type signature)` for every
  named item, the same names the disk store and `.ambient` artifacts
  already record.

A **deploy** applies a generation to a running system, in this order:

1. **Load** the objects additively into the runtime's stores and VM
   tables. Never destructive: old hashes stay resident.
2. **Validate.** Every name that exists in both the old and new tables
   is checked for signature compatibility (see the Live ability's
   rebinding rule), and every pending state migration is checked
   against its cell (see "State cells"). Failures reject the deploy
   here — loudly, with the previous generation untouched.
3. **Swap** the name table atomically. From this instant, every
   late-bound resolution sees the new generation.
4. **Reconcile**: run the new entry function. The entry is a
   _declaration_, not a program — it initializes cells (which adopt),
   ensures tasks (which no-op if alive), and returns. Tasks the entry
   no longer declares are drained.
5. **Report** the diff, which content addressing makes exact: names
   rebound, names unchanged (same hash — identical subtrees are
   skipped by construction), tasks started/drained, cells created/
   migrated/adopted, plus the diagnostics below.

Deploys are produced by frontends, and the runtime does not know which:
the dev loop (`ambient dev`, from the file watcher), the REPL
(redefining an Fqn produces a one-item generation), and remote deploy
(the same pack received over the wire and applied via the `Deploy`
ability — see [remote-execution.md](remote-execution.md)). One
mechanism, three producers.

## The Live ability

Reading the current generation is a side effect — it is
nondeterministic across time, exactly like `Random::next!` — so it is
an ability:

```ambient
pub unique(…) ability Live {
  /// The current binding of the name `f` was deployed under, or `f`
  /// itself when no deploy runtime is present.
  fn latest<F>(f: F): F { f }
}
```

- **Upgradeability is visible in effect rows.** `with Live` in a
  signature says this code participates in hot replacement. Pure
  functions stay pinned and referentially transparent; a sandbox can
  deny `Live`; a test can install a handler that pins a fake
  generation.
- **The default implementation is identity.** Under plain
  `ambient run` every `latest!` resolves to the compile-time ref and
  the program behaves identically, minus liveness. Only a deploy
  runtime installs the real handler.
- **Resolution is by deployed name.** The argument is an ordinary
  function ref (a hash); the runtime maps hash → the Fqn it was
  deployed under → that name's current hash. A ref that was never
  deployed under a name (a capture-free lambda, a synthetic symbol)
  resolves to itself.
- **Rebinding is same-signature-only.** Deploy validation compares
  canonical signatures: a name whose signature changed is _not_ a
  rebinding — the old binding retires, the name enters the table fresh,
  and the deploy reports it (`latest` sites keep resolving to the old
  ref until their own callers upgrade). Evolving a live boundary's
  signature therefore takes a new name or a shim — deliberately the
  discipline of a database migration.
- **Consistency comes from single reads, not read barriers.** One
  `latest!` read returns a hash-pinned ref whose entire subtree is
  internally consistent. Two separate reads may straddle a deploy; the
  convention is one `latest!` read per unit of work (per request, per
  reduction, per tick), taken at the top. The `latest` is typed as a
  bare generic for the same reason `Process::spawn`'s parameters are —
  ability signatures cannot yet express effect-polymorphic function
  parameters — and the runtime checks it received a function ref.

The canonical loop idiom — Erlang's fully-qualified `?MODULE:loop(State)`
call, spelled as an effect:

```ambient
fn accept_loop(): () with Http, Live {
  let handle = Live::latest!(handle_request);   // fresh per iteration
  handle(need(Http::accept!(listener)));
  Live::latest!(accept_loop)()                   // the loop upgrades itself
}
```

## State cells

The process experiment's one durable truth was never the mailbox; it
was _"the runtime owns the state, and user code touches it only through
a boundary."_ Cells keep the truth and drop the mailbox: a **cell** is
a named, typed, runtime-owned value that belongs to no generation.

```ambient
pub unique(…) ability State {
  /// Adopt the cell if it exists, else create it with `make`.
  fn init<S>(name: String, make: () -> S): ();
  /// Adopt, migrate, or create (see "Migration").
  fn init_versioned<Old, New>(name: String, make: () -> New, migrate: (Old) -> New): ();
  fn get<S>(name: String): S;
  fn set<S>(name: String, value: S): ();
  /// Atomic read-modify-write; returns the new value.
  fn update<S>(name: String, f: (S) -> S): S;
}
```

- **Adoption is the point.** `init` on an existing cell is a no-op, so
  the entry can re-run on every deploy. Editing a cell's `make`
  initializer changes nothing for live state — only a migration does.
- **Cells are why there is no state handoff.** Old code's Exit phase
  and new code's Init phase collapse into "the cell is still there":
  state never left the runtime, so there is nothing to serialize, ship,
  or negotiate.
- **IO handles live in cells.** Host resources are already opaque
  handles into runtime-owned tables shared across all VMs and
  generations (the network handle table is the existing precedent). A
  listener bound by generation 1 and stored in a cell is served by
  generation 40 with no ceremony: the runtime owns both the name and
  the resource, and can trace both.
- **Cells are trace roots** for retirement (below), and the runtime can
  say exactly which cell pins which generation.

### Migration

Cells remember the static type they were last written at — a
**fingerprint** of the writer's canonical type, threaded through the
perform by the compiler. `init_versioned<Old, New>` dispatches on it:

| Cell state          | Action                                      |
| ------------------- | ------------------------------------------- |
| no cell             | `make()`                                    |
| fingerprint = `New` | adopt (the every-deploy no-op)              |
| fingerprint = `Old` | `migrate(old)`; fingerprint advances        |
| anything else       | **deploy rejected** at validation, pre-swap |

The contract is deliberately the pragmatic version: **new code types
the old value.** A shape change keeps the previous declaration in the
source as a relic (`StatsV1`), and the migration is an ordinary typed
function from relic to current. Retirement tracing tells you when no
live cell holds the relic shape anymore — i.e. when the relic can be
deleted. A derived-from-the-store old type (à la Unison) is a possible
future refinement, not part of this design.

Mechanics, as implemented: the fingerprint is the canonical type
rendering ability signatures already hash (byte-stable, uuid-keyed
nominals, position-canonical variables). State's write-path methods
declare trailing `fingerprint: String` parameters that call sites never
spell — the checker (anchored on the reserved State uuid) hides them
from perform-site arity, solves the cell type by constraining
`make`/`migrate`/`f` to function shapes, and the compiler pushes the
renderings as hidden trailing arguments, the trait-dictionary shape.
Two consequences fall out:

- The pre-swap reject arm is only statically checkable for **literal
  cell names**: those perform sites ship a `(cell, old, new)` record in
  the generation, which validation checks against the live cell table
  before the swap. An `init_versioned` whose name is computed validates
  at perform time instead — the mismatch is a fault during
  reconciliation, not a rejection.
- A cell write whose type mentions an enclosing type parameter is a
  check error: a fingerprint must mean exactly one type per site, and
  compilation is dictionary-passing, never monomorphizing — there is no
  per-instantiation type to thread.

## Tasks

A **task** is a named, supervised, drainable loop — much less than a
process: no mailbox, no reducer contract, no message types.

```ambient
pub unique(…) ability Task {
  /// Ensure a named task is running. No-op if alive.
  fn ensure(name: String, body: () -> ()): ();
  /// Ask a task to unwind at its next interruptible perform.
  fn drain(name: String): ();
}
```

- Tasks are **reconciled by name**: `ensure` during a deploy pass
  matches the live registry; a task the entry stops declaring is
  drained (not killed).
- The runtime never swaps a task's code. Tasks keep _themselves_ fresh
  by re-entering through `Live::latest!` — the loop idiom above — which
  removes the staged-swap machinery the process reconciler needed.
  A task whose loop recurses directly instead is pinned forever, and
  deploy diagnostics say so.
- Each task runs on its own thread with its own VM; all IO stays
  blocking; the shared handle and cell tables are the only
  cross-task state. Fault handling follows the process runtime's
  precedent (restart on fault, park after a fault budget).

Mechanics, as implemented: the language has no tail calls (and a
bounded call depth), so the loop idiom above cannot yet be spelled in
Ambient — **the task runtime is the loop**
(`crates/ambient-platform/src/task.rs`). A task body is one bounded
pass; the runtime re-invokes it forever, resolving the body's deployed
name against the current generation before every pass (exactly the
`live_latest` resolution) and topping the task's VM up with any newly
loaded generation first. The visible consequences:

- Editing a task's body (or anything below it) lands on the very next
  pass, with no restart and no re-declaration — `ensure` on a live
  name is a no-op even when the declared body hash changed, because
  freshness comes from the per-pass resolution, never from
  re-ensuring. A closure body has no deployed name and stays pinned.
- Author-placed `Live::latest!` points remain meaningful _inside_ a
  pass (per-connection dispatch under a per-pass accept, the
  `examples/live_site` acceptor's shape).
- How a pass ends classifies the task's fate: a pass completing with a
  drain requested (its `Drain::requested` arm ran) is a clean drain;
  an unwind surfacing unhandled is drained-without-cleanup; a hard
  stop (`VmError::HardStopped`, the drain deadline) always parks —
  never restarts; any other fault restarts the pass (the retry
  re-resolves, so a deploy can fix a crash loop) until five
  consecutive faults park the task.
- A drain (`Task::drain!` or an undeclaring deploy) frees the name
  immediately; the winding-down task keeps running to its next
  interruptible perform, so a redeploy may reuse the name while the
  old task drains.

When tail calls land, the in-language idiom can replace the runtime
loop without changing the ability's surface.

## Drain

Cooperative cancellation reuses machinery the VM already has:
never-returning methods unwind and discard the delimited continuation.

```ambient
pub unique(…) ability Drain {
  fn requested(): !;
}
```

- The runtime delivers `Drain::requested!` **only at interruptible
  performs** — a marked subset of blocking platform operations
  (`accept`, `receive`, `receive_raw`, `wait`). Between
  interruptible performs, code runs to completion: invariants never
  tear. These are reduction boundaries, generalized and author-placed —
  the author chooses the granularity by choosing where the blocking
  calls sit.
- Cleanup is structured code, not a signal handler: the nearest
  `Drain::requested` arm receives the unwind and yields the
  computation's final value.

```ambient
with { Drain::requested() => checkpoint_and_report() }
  handle serve_loop()
```

- In-flight work is bounded by checkpoints: the unwind discards the
  continuation, so loops persist what matters into cells as they go —
  the same contract as Erlang's kill-at-reduction-boundary, with
  explicit, delimited cleanup.
- A drain carries a **deadline**. A computation that does not reach an
  interruptible perform in time is hard-stopped at the runtime's next
  opportunity and parked, like a fault-budget exhaustion — a broken
  code path cannot wedge a deploy.
- Drain makes itself rare: an acceptor that dispatches each unit of
  work through `Live::latest!` never drains for a _code_ upgrade at
  all; it drains only when its resource (the listener itself) must go
  away.

Mechanics, as implemented: a `DrainSignal` is the per-computation
handle a draining host holds (the raw hook the `Task` registry
drives). Wiring a VM with `install_drain_natives` overrides the
interruptible subset — `network_accept`, `network_receive`,
`network_receive_raw`, `time_wait` — with variants that race the
blocking operation against the signal;
an interrupted native returns the engine's `VmError::Interrupted`
carrying the anchors in `ambient_core::drain` (the Exception-anchor
precedent), and the VM performs `Drain::requested!` as a
host-constructed suspended never value at the native's own call site.
The signal is one-way: every interruptible perform after a request
unwinds immediately, so the delivery point is deterministic. Three
consequences fall out:

- Drain does **not** appear in effect rows: the delivery rides the
  runtime channel (like a native's exception), so `Time::wait`'s
  signature is unchanged and a handler arm may wrap any body. An
  unhandled delivery is an unhandled-ability fault (`requested` is
  abstract — the never carve-out), which the draining host reads as
  "drained without cleanup".
- The deadline is a watchdog thread plus the VM's host interrupt flag:
  expiry hard-stops the computation at the next opcode boundary
  (`VmError::HardStopped`, checked every 64 opcodes). A native blocked
  in the host past the deadline is _not_ interrupted — only opcode
  boundaries observe the flag, so a non-interruptible native that never
  returns wedges its thread (don't write one).
- An interrupted `receive` may abandon a partially-read frame; the
  connection's framing is undefined afterwards. Fine by construction:
  a drain means the resource is being torn down.

## Retirement

"When is the upgrade finished?" is exactly computable here, which
almost no live-update system can claim. Old code is reachable from
precisely three kinds of place, and the runtime owns all of them:

- **in-flight frames** — die at the next boundary;
- **cells** — may hold `FunctionRef`s, closures, or handler values;
- **task/registry state** — same.

Generation retirement is a trace: roots = live frames ∪ cell values ∪
registry values → reachable hashes → a generation none of whose hashes
are reachable is **retired** — reported by the dev loop, purgeable by
`ambient store gc`. Better than a boolean, the trace is _diagnosable_:
the runtime can report "generation 1 is pinned by cell `conn-42`,
which holds a closure of `handle_connection` v1" — naming the value
that refuses to migrate. (Erlang's answer is a hard cap of two
versions and killing lingerers; a configurable cap can layer on top of
tracing for deployments that want the guarantee.)

This is also why the closure rule exists: **names for durable roles,
closures for ephemeral work.** A closure is code fused to captured
state; content addressing can only rebind the code half. A closure
stored in a cell is never a correctness hazard (it stays pinned and
keeps working) — it is a liveness hazard: it pins its generation until
it is dropped.

Mechanics, as implemented (`crates/ambient-platform/src/retire.rs`):
the deploy runtime records every successful swap as a numbered
generation and attributes each object hash to its **latest shipper** —
a full build re-ships every unchanged hash, so unchanged code
attributes to the new generation, and an old generation stays live only
while a hash it _alone_ still ships is reachable. Roots are gathered,
not sampled from live VMs: the cell table, each registry's contribution
(a task publishes the hash it resolved for the current pass — between
boundaries its frames can hold only code reachable from that hash plus
already-rooted values; a process publishes its reducers and its state
as of the last reduction), and the current name table. The trace's BFS
carries provenance, so a pin names its most direct holder. Retirement
is **sticky**: unreachable code cannot come back into reach, so the
transition is reported once and recorded forever. Consequences:

- A task's ensure-time body hash is a resolution key, not a root — a
  named task never pins the generation that ensured it. A closure body
  is the running code and does pin.
- The one known hole is process mailboxes: a closure inside an
  undelivered message is invisible until it is reduced into published
  state. Tasks and cells have no such channel.
- The dev loop gc's a package's `.ambient` store after each deploy with
  the trace's reachable set as extra roots, so a pinned old
  generation's objects survive on disk exactly as long as something
  live holds them; `ambient store gc` (offline) keeps using the names
  index as roots.
- Programmatic access is `DeployRuntime::retirement()`; the dev loop
  prints the transitions and pins per deploy.

## What is live and what is pinned

| Thing                                | Fresh when…                                                                               |
| ------------------------------------ | ----------------------------------------------------------------------------------------- |
| The entry function                   | every deploy (it is the reconciliation pass)                                              |
| Anything reached via `Live::latest!` | the next call through that point                                                          |
| A task's loop body                   | the next iteration, if the tail call goes through `latest!`                               |
| A `const`                            | whenever a function reading it is fresh (the new function hash embeds the new const hash) |
| Cell contents                        | never, by design — only a migration changes live state                                    |
| A closure held in a cell or registry | never — pinned to its hash; prefer names at durable boundaries                            |
| Code above every late-bound point    | never — and deploy diagnostics say so                                                     |

## Deploy diagnostics

Content addressing lets the deploy pass _see_ the failure modes other
systems hit silently. Validation and the deploy report include:

- **Unreachable change**: a changed item that no live re-entry point
  can ever reach — "`accept_loop` changed but the running copy can only
  retire on restart." The nudge is: add a `latest!` point or restart.
- **Uncovered method key**: ability evolution re-keys methods
  (`MethodKey` includes the default implementation), so old code's
  performs cannot match handlers compiled for the new key. That is
  sound — old performs fall through to the old defaults, which shipped
  with the old code — but silent, and it is the one channel where
  behavior can drift without a loud failure. The deploy warns when a
  live generation performs a key no current handler covers.
- **Signature-changed name**: reported as retire-and-fresh, never as a
  rebinding (see the Live rebinding rule).

Mechanics, as implemented: warnings ride `DeployReport::warnings`,
computed after reconciliation from the same trace machinery. The
unreachable-change warning has two flavors: **severed** (the old copy's
lineage retired — its own late binding resolves to itself forever, the
signature-changed-task-body case) and **orphaned** (the rebinding is
fine but no live re-entry point — the entry, a task body's per-pass
resolution — reaches the new code). Only task roots count as
late-bound re-entry points: the runtime genuinely re-resolves them,
while a cell-held value's hypothetical `latest!` cannot be seen
statically. The uncovered-key warning fires for keys performed by
strictly-old live code that current code neither performs nor covers,
scoped to abilities the fresh generation covers at all — otherwise
every old perform of a default-implemented, never-handled ability
(`State`, `Time`, ...) would warn. A pinned-but-deliverable rebinding
warns as nothing: it is a retirement _pin_, which the dev loop prints
alongside the warnings.

## Non-goals

- **Implicit late binding.** Making every call a name lookup would
  recover Smalltalk's liveness by destroying the hash-pins-behavior
  guarantee, the perf model, and the ability to reason about a subtree
  from its root hash. Late binding stays explicit and effect-tracked.
- **Upgrading in-flight frames.** Boundaries are the answer; nobody
  ships the alternative.
- **State serialization handoff between VMs.** Cells make local handoff
  a no-op; the remote path ships generations (code), never live state.
- **A blessed server or UI framework.** The runtime provides
  generations, `Live`, cells, `Task`, `Drain`, and the retirement
  trace. Frameworks own policy: what drains when, what a component key
  is, route-level versus connection-level swap. Individual code paths
  with bespoke requirements compose the same primitives.

## Relation to the process model

Inherited from the experiment (see [processes.md](processes.md)),
generalized out of it:

- the **deploy pass / reconcile-by-name** loop (from the process
  registry to the whole name table);
- **runtime-owned state** (from reducer state to named cells);
- the **shared handle table** (already generation-agnostic; now a named
  invariant).

Dropped: the mailbox/reducer boundary as _the_ unit of upgrade, staged
reducer swaps (self-upgrade through `latest!` replaces them), and the
root/dynamic process split (tasks + pinned closures cover both cases
without a mode). Whether a mailbox/reducer library survives as _a_
concurrency story is a separate decision; if it does, it is an ordinary
client of cells, tasks, and generations.

## Open questions

- **Snapshot groups.** Single-read consistency covers the common case;
  a construct for "these two `latest!` reads must agree" (resolve both
  against one snapshot) may be wanted eventually.
- **Cell namespacing.** Cell names are plain strings by convention
  (hierarchical, `"app::stats"`); collisions between frameworks sharing
  a runtime are unguarded. An `Fqn`-scoped or capability-scoped naming
  scheme is future work.
- **Typed `latest` / typed task bodies.** Both fall to the same gap as
  typed `spawn`: effect-polymorphic function parameters in ability
  signatures.
- **`latest!` on lambdas with captures** is resolved-to-itself today
  (no deployed name). Rebinding code under a kept environment is
  semantically defensible and deliberately excluded.
