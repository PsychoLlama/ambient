## Structure

- `crates/*`: Language implementation across several crates. Special cases noted below.
  - `ambient-cli/`: CLI for managing ambient codebases.
  - `ambient-platform/`: Bindings for performing effects on the host system.
  - `ambient-lsp/`: Language server.
- `ambient.nvim/`: Neovim plugin providing basic language support.
- `tree-sitter-ambient/`: Syntax highlighting.

## References

- `ref/architecture.md`: source of truth for language design.

## Committing

- Run `just check` and keep it passing.
