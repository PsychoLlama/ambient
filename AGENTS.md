## Structure

- `spec/`: source of truth for the language design. The implementation follows it; change the spec first.
- `crates/ambient`: the implementation, a library plus an empty binary. Module docs name the spec page each module implements.
- `crates/ambient-runtime`: what compiled code calls. Built natively into the compiler, and for wasm32 by `crates/ambient/build.rs`.

## Invariants

- No text syntax. Programs are made with `ambient::builder`; do not add a parser or a printer that can be parsed back.
- Every stage result is stored under a key derived from the content of its inputs (`engine/mod.rs`). A stage's compute function reads nothing that is not in its key.
- Checking is re-entrant: a comptime call compiles and runs its callee mid-check (`engine/run.rs`). The check key covers the callee's hash, which is what keeps the result a function of the key.
- A result that observed the world is stored with what it observed and reused through `Engine::stage_if`, which looks again. Only native code is outside a key, so a trace is the native calls a run made.
- Names, docs, and declaration order never reach a core tree or a hash.
- A backend only translates `lir`; anything about semantics belongs in `lower.rs` or the runtime, so both backends get it.
- Layouts and the calling convention are defined once, in `ambient-runtime`; never restate an offset in a backend.
- Every function's first parameter is its bindings: a record keyed by ability, or null. The checker tracks what is bound (`checker/abilities.rs`); lowering only threads the record (`lower/abilities.rs`).
- A row is ordered by tag wherever it reaches a core tree, never by name or by the order it was written in.
- A program is `Engine::program(entry, target)`: a root function the engine writes (`engine/program.rs`) and then checks, hashes, and compiles like any other.
- Compiled code reaches native code only through `amb_native` and an index the loader fills in; a host supplies `Natives` by package and tag.
- A task suspends by unwinding with the context's mode set, and resumes by its functions being called again (`lower/resume.rs`, `ambient-runtime/src/task.rs`). A host that sees the entry unwind while suspending calls `amb_schedule` and the entry again.
- A task's body is a closure that takes nothing; the checker makes one for each branch, spawn, and cleanup (`checker/tasks.rs`), so they capture locals and bindings as any closure does.
- Whatever must happen on every way out of a block (cleanups, closing a group) is lowered as a `Try` whose recovery holds the unwinding, does the work, and rethrows (`lower/tasks.rs`).
- Metering is a property of lowering (`lower/meter.rs`), chosen for a whole program at link time: a program that enters a budget anywhere is compiled metered throughout, under the same hashes.
- Compiled code is position-independent: it reaches definitions through slots and the runtime through entry points, never by address.
- `core::Tree::encode` defines every hash. Changing it or the node set means bumping `FORMAT`.

## Testing

- Tests build a `Workspace` in memory and run an `Engine` over a `MemoryStore`; nothing touches disk.
- A program is run with `support::run` or `support::run_with`, which run it natively and in wasmtime and fail if they disagree or if either leaks an object.
- An incremental test builds twice over one store and asserts on `Engine::ran` and `Engine::reused`.

## Committing

Keep all of these passing:

- `treefmt --ci` (`treefmt` to fix).
- `cargo clippy --workspace --all-targets --quiet`. Lints live in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- `cargo nextest run --workspace`.
