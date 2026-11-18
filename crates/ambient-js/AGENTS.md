# ambient-js

The tests of the JavaScript target, `packages/js`, whose `AGENTS.md` holds its invariants. The root `AGENTS.md` holds what spans crates.

## Testing

- `tests/host.rs` builds a program with the target, as `ambient build --target` does, into a directory of its own, and runs `js/*.test.js` under Node's own runner against what was built.
- The tests are thin: one fixture program, one to load into with a unit, and one Node process. What a program means is tested in `crates/ambient`.
- Nothing here is installed from a registry. Keep it so.
