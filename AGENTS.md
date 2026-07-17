## Structure

- `crates/*`: Language implementation across several crates. Special cases noted below.
  - `ambient-cli/`: CLI for managing ambient codebases.
  - `ambient-platform/`: Bindings for performing effects on the host system.
  - `ambient-analysis/`: Shared parse/check/diagnose pipeline behind both `ambient check` and the LSP.
  - `ambient-lsp/`: Language server.
- `ambient.nvim/`: Neovim plugin providing basic language support.
- `tree-sitter-ambient/`: Syntax highlighting.

## Invariants

Rules only — mechanism and rationale live in the module docs at each path.

- The LSP is a renderer: deciding what is an error or resolving a name belongs in `ambient-analysis` or the engine, never `ambient-lsp`. `ambient-lsp/tests/parity.rs` pins LSP diagnostics == `ambient check`; fix the shared layer, don't fork a frontend.
- IDE paths parse with `ambient_parser::parse_recovering`; only the compiler uses fail-fast `parse`.
- Every item has one identity: the `Fqn` struct (`ambient-engine/src/fqn.rs`), stamped on every reference — same-module included — by the resolve pass (`src/resolve/`). Tables key off `Fqn`/`NameKey` equality: never string comparison, never a second lookup convention.
- Only lexical locals and `<uuid>::method` dispatch symbols stay `NameKey::Bare` (registry-less paths — single-file, some tests — skip resolve and stay bare throughout). Content hashes never take the `Fqn` as input (`compiler/hash.rs`).
- A name resolves iff something in scope put it there — prelude, `use`, or local declaration; no name-keyed builtin fallbacks (containers and `Exception` are ordinary `core` declarations, re-exported by `core_lib/prelude.ab`). Type syntax (arrows, tuples, `Handler<A, R>`) is not a name and is exempt.
- Manifest-backed packages always **mount**: module paths lead with the package name (the root `main.ab` collapses to the mount as a directory module), `ModuleRegistry::module_id`/`module_path_of`/`module_key` are the only mount↔scope conversions, and `pkg::`/`super` never cross the mount. Workspace members (`workspace.rs`; `Workspace::discover` is the single upward-walk authority) share one registry per build and one `.ambient` store at the workspace root with per-package snapshot pointers. `ambient-cli/tests/workspaces.rs` pins the surface.
- `ambient-cli/tests/module_system.rs` pins the access rule: anything reachable fully-qualified works through `use`, and vice versa.
- Cross-module compile channels come only from `ModuleEnv` (`src/module_env.rs`), whose walk functions are shared with the checker's import registration. Never hand-assemble a channel at a call site or re-implement a walk.
- `extern fn` is the host boundary: `.ab` declarations own signatures/docs; hosts bind implementations by stable UUID in a `NativeRegistry` (renames re-key the binding loudly via `verify_native_contract`, but never move a hash). Registry natives are pure value transformations; effectful ones stay module-private so ability default implementations are their only callers.
- Never reuse or renumber a reserved UUID: core/platform native blocks (`natives/core/mod.rs`), operator-trait uuids (`types/traits.rs`), and platform ability ids are all golden-test-pinned.
- Traits and abilities are nominal: the mandatory `unique(<uuid>)` is the identity, and coherence, dispatch symbols, bounds, and operator desugaring key off it — never the name. `Infer::trait_uuid_of` is the single trait-resolution entry point; primitives are exempt from operator-trait dispatch (their operators are builtin opcodes).
- Parameterized traits (`From<T>`) key coherence and dispatch symbols on each trait argument's _head_ uuid; every argument must have a nominal head. The conversion bridges (`S: Into<T>` iff `impl From<S> for T`; `S: TryInto<T>` iff `impl TryFrom<S> for T`) anchor on the reserved uuid pairs and are sound only because `validate_reserved_trait` pins each pair to one dictionary shape (the try pair's pin includes the associated `Error`) — never change either half of a pair alone.
- Associated types (`type Error;`) are checker-only vocabulary: bindings live on `TraitImpl`, `Self::X` in a trait signature is a `Type::Projection` eliminated by the dispatching impl's binding (rigid — `Param`-like — when no impl is known), and nothing about dispatch symbols, dictionaries, coherence keys, or content hashes may ever depend on them.
- Trait bounds compile by dictionary passing, never monomorphization; `ast/items.rs::dict_params` is the single authority on dictionary order/count. Every body-check site must call `finish_body_constraints` so pending constraints never leak across bodies.
- An ability method's identity is its `MethodKey` (`ambient-core/src/identity.rs`): renames are free, signature or default-implementation edits re-key loudly. Canonical signature rendering must go through `ability_id_infer`'s seeded context (`infer/check/abilities.rs`); a fresh `Infer::new()` corrupts method keys.
- Every ability method carries a default implementation, compiled under its `<ability-uuid>::<method>` dispatch symbol through the ordinary linking table — never add a parallel channel. Only never-returning methods (`: !`) may stay abstract; an unhandled abstract perform is a runtime fault.
- Engine special cases key on uuid anchors, never names: `EXCEPTION_UUID` with the pinned `THROW_IMPL_HASH` (`ambient-core/src/exception.rs`; when its golden test fails, update the literal it reports), and `STATE_UUID` for compiler-supplied fingerprints (`infer/fingerprints.rs`; never add a second fingerprint encoding).

## References

- `ref/architecture.md`: source of truth for language design.
- `ref/*.md`: per-subsystem design (abilities, traits, types, modules, live-upgrade, remote-execution, core-library).

## Committing

- Run `just check` and keep it passing.
- `just check` enforces per-file line budgets (`scripts/file-size-budgets.txt`): new `.rs` files cap at 1000 lines; grandfathered files may only shrink. Split a file instead of growing it — never add a budget entry for new growth.
