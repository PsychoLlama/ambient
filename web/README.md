# The JavaScript host

`host.js` loads a program built with `ambient build --module <target>` and runs it in a browser or in Node. It is one module with no dependencies.

- A program's target binds abilities to externs. The host is handed a function for each, by the package's ID and the extern's module path and name.
- A function that gives a promise answers later. Its extern must be declared `suspending`.
- The tests are `test/*.test.js`, run by Node's own runner. `cargo nextest run` builds `examples/browser` and runs them (`crates/ambient/tests/web.rs`).
- Nothing here is installed from a registry. Keep it so.

See "WebAssembly hosts" and "JavaScript hosts" in `spec/targets.md`.
