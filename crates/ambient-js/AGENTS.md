# ambient-js

The JavaScript host of a module built with `ambient build --module`. The root `AGENTS.md` holds what spans crates.

## Invariants

- `js/host.js` is ES modules with no dependencies, which know of no browser and no Node. Install nothing from a registry.
- `constants.js` is written from the runtime by `build.rs` and never committed; restate no constant in JavaScript.
- The host restates what `backend/wasm`'s host does; where they differ, that one is right.
- A run keeps its own waits, by its context (`Waits`), so runs at once never share an answer.

## Testing

- `ambient-js/tests/host.rs` builds a program as a module and runs `js/*.test.js` under Node's own runner. JavaScript is tested nowhere else, and only what the host itself does.
- The tests are thin: one fixture program, one build, one Node process. What a program means is tested in `crates/ambient`.
