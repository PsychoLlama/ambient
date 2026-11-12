## Structure

- `spec/`: source of truth for the language design. The implementation follows it; change the spec first.
- `crates/*`: the implementation, one cargo workspace.

## Committing

Keep all of these passing:

- `treefmt --ci` (`treefmt` to fix).
- `cargo clippy --workspace --all-targets --quiet`. Lints live in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- `./scripts/check-file-sizes.sh`. `.rs` files cap at 1000 lines: split a file, never add a budget.
- `cargo nextest run --workspace`.
