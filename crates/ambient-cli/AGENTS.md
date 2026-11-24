# ambient-cli

The `ambient` binary. The root `AGENTS.md` holds what spans crates.

## Invariants

- `src/main.rs` only wires the world together: a store, snapshots, a linker, and the launcher (`LAUNCHER`, from `ambient-archive`). Every decision is the library's (`ambient::cli`).
- Its tests run the built binary as a process, under the system's temporary directory. Anything that can be tested through `ambient::cli::Session` is tested in `crates/ambient` instead.
