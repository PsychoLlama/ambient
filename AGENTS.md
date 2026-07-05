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

## References

- `ref/architecture.md`: source of truth for language design.

## Committing

- Run `just check` and keep it passing.
