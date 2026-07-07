## Structure

- `crates/*`: Language implementation across several crates. Special cases noted below.
  - `ambient-cli/`: CLI for managing ambient codebases.
  - `ambient-platform/`: Bindings for performing effects on the host system.
  - `ambient-analysis/`: Shared parse/check/diagnose pipeline behind both `ambient check` and the LSP.
  - `ambient-lsp/`: Language server.
- `ambient.nvim/`: Neovim plugin providing basic language support.
- `tree-sitter-ambient/`: Syntax highlighting.

## Invariants

- The LSP is a renderer: anything that decides *what is an error* or resolves a name belongs in `ambient-analysis` or the engine (`ModuleRegistry`), never in `ambient-lsp`. Do not add LSP-private indexes or resolvers.
- `crates/ambient-lsp/tests/parity.rs` pins LSP diagnostics == `ambient check` diagnostics. If it fails, fix the shared layer — don't fork behavior in a frontend.
- IDE paths parse with `ambient_parser::parse_recovering` (partial AST + all errors); only the compiler uses fail-fast `parse`.
- Every item has one fully-qualified identity — a real `Fqn` struct (`crates/ambient-engine/src/fqn.rs`): a `Scope` (`Builtin` = `core`, or `Workspace(pkg)`) + module path + ident path, rendered `core::primitives::Number::sqrt` / `workspace::pkg::math::gcd` (`::` is the sole path separator, `.` is value/field access). The engine's resolve pass (`crates/ambient-engine/src/resolve.rs`) canonicalizes every resolved reference — *same-module included* — to its `Fqn` before checking (an enum variant is the two-segment ident `[Enum, Variant]`). The checker env, intrinsic tables, ability resolver, and linker key off the `Fqn` struct (via `NameKey`'s `Eq`/`Hash`) — never string equivalence, and never a second lookup convention or a spelling-specific code path. Only two things stay `NameKey::Bare`: true lexical **locals**, and content-addressed method-dispatch symbols (`<uuid>::method`). Content hashes never depend on the `Fqn` — `finalize_module_hashes` folds a recursive function's *bare* short name into its group object, so the `Fqn` is a compile-time lookup key and post-hash label, never an input to content addressing. `Display` is for humans and the on-disk store only. (Registry-less checks/compiles — single-file, some tests — never run resolve, so their own items stay bare; a `None` module id threads that through the checker/compiler.)
- `crates/ambient-cli/tests/module_system.rs` pins the access rule: anything reachable fully-qualified works through `use`, and vice versa.

## References

- `ref/architecture.md`: source of truth for language design.

## Committing

- Run `just check` and keep it passing.
