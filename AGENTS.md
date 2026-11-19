## Structure

- `spec/`: source of truth for the language design. The implementation follows it; change the spec first.
- `crates/ambient`: the implementation, a library plus an empty binary. Module docs name the spec page each module implements.
- `crates/ambient-host`: what loads and runs a compiled program, with no compiler in it: trees, values, parts, the native loader, the native side of `system`, and launching. `ambient` gives its modules again at the paths they had (`ambient::core`, `backend::native`, `host`), which is how the bullets below name them.
- `crates/ambient-launcher`: the binary a built program is put after. It links `ambient-host` and nothing of the compiler; keep it so.
- `packages/js`: the JavaScript target, an Ambient package: the tool that builds a program for a JavaScript host, and the host (`host/host.js`). `crates/ambient-js` holds its tests.
- `integrations/neovim`: a Neovim plugin for `.ab` files: filetype, highlighting by pattern, and comment settings. It follows `spec/syntax.md` by hand; change it when the syntax changes.
- `crates/ambient-runtime`: what compiled code calls. Built natively into the compiler, and for wasm32 by `crates/ambient/build.rs` as one relocatable object.
- Each crate but the launcher, and each package, has an `AGENTS.md` of its own invariants. Read it before working there, and put a new invariant in the crate that must uphold it.

## Invariants

What spans crates:

- A backend only translates `lir`; anything about semantics belongs in `lower.rs` or the runtime, so both backends get it.
- Layouts and the calling convention are defined once, in `ambient-runtime`; never restate an offset in a backend.
- Compiled code reaches native code only through `amb_native` and an index the loader fills in; a host supplies `Natives` by package and the extern's module path and name.
- A host that sees the entry unwind while suspending calls `amb_schedule` and the entry again, on either backend: natively `amb_start` runs the root on a stack and that second call resumes it.
- Compiled code is position-independent: it reaches definitions through slots and the runtime through entry points, never by address.

## Testing

- Tests build a `Workspace` in memory and run an `Engine` over a `MemoryStore`; nothing touches disk, except `tests/disk.rs` and `tests/fetched.rs`, which test the `DiskStore` and fetching with `git` under the system's temporary directory.
- Programs are built with builders. Text is for tests of the reader, of library methods (`support::text`), and of whole packages.
- A program is run with `support::run` or `support::run_with`, which run it natively and in wasmtime and fail if they disagree or if either leaks an object.
- An incremental test builds twice over one store and asserts on `Engine::ran` and `Engine::reused`.
- JavaScript is tested only in `crates/ambient-js`, against a program built by `packages/js`.

## Committing

Keep all of these passing:

- `treefmt --ci` (`treefmt` to fix).
- `cargo clippy --workspace --all-targets --quiet`. Lints live in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- `cargo nextest run --workspace`.

## Releasing

- A release is a tag `v` and the workspace's version in `Cargo.toml`, pushed: `.github/workflows/release.yml` tests, builds, and tries the command on each machine, then attaches the binaries to a GitHub release.
- Run by hand (`gh workflow run release.yml`), the workflow does all but publish. Do that before tagging when a machine CI does not cover may have broken.
