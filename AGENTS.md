## Structure

- `spec/`: source of truth for the language design. The implementation follows it; change the spec first.
- `crates/ambient`: the implementation, a library plus an empty binary. Module docs name the spec page each module implements.
- `crates/ambient-runtime`: what compiled code calls. Built natively into the compiler, and for wasm32 by `crates/ambient/build.rs`.

## Invariants

- No text syntax. Programs are made with `ambient::builder`; do not add a parser or a printer that can be parsed back.
- Every stage result is stored under a key derived from the content of its inputs (`engine/mod.rs`). A stage's compute function reads nothing that is not in its key, and never sees the engine.
- Names, docs, and declaration order never reach a core tree or a hash.
- A backend only translates `lir`; anything about semantics belongs in `lower.rs` or the runtime, so both backends get it.
- Layouts and the calling convention are defined once, in `ambient-runtime`; never restate an offset in a backend.
- Compiled code is position-independent: it reaches definitions through slots and the runtime through entry points, never by address.
- `core::Tree::encode` defines every hash. Changing it or the node set means bumping `FORMAT`.

## Testing

- Tests build a `Workspace` in memory and run an `Engine` over a `MemoryStore`; nothing touches disk.
- A program is run with `support::run`, which runs it natively and in wasmtime and fails if they disagree.
- An incremental test builds twice over one store and asserts on `Engine::ran` and `Engine::reused`.

## Committing

Keep all of these passing:

- `treefmt --ci` (`treefmt` to fix).
- `cargo clippy --workspace --all-targets --quiet`. Lints live in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- `cargo nextest run --workspace`.
