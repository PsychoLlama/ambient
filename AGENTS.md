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
- Every item has one fully-qualified identity — a real `Fqn` struct (`crates/ambient-engine/src/fqn.rs`): a `Scope` (`Builtin` = `core`, or `Workspace(pkg)`) + module path + ident path, rendered `core::primitives::Number::sqrt` / `workspace::pkg::math::gcd` (`::` is the sole path separator, `.` is value/field access). The engine's resolve pass (`crates/ambient-engine/src/resolve.rs`) canonicalizes every *cross-module* reference to its `Fqn` before checking. The checker env, intrinsic tables, ability resolver, and linker key off the `Fqn` struct (via `NameKey`'s `Eq`/`Hash`) — never string equivalence, and never a second lookup convention or a spelling-specific code path. A module's own items (and content-addressed method symbols) stay bare (`NameKey::Bare`); only genuine cross-module identity is an `Fqn`. `Display` is for humans and the on-disk store only.
- `crates/ambient-cli/tests/module_system.rs` pins the access rule: anything reachable fully-qualified works through `use`, and vice versa.

## References

- `ref/architecture.md`: source of truth for language design.

## Committing

- Run `just check` and keep it passing.
