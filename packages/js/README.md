# The JavaScript host

The JavaScript is `js/`; the Rust around it writes the constants and runs the tests.

`js/host.js` loads a program built with `ambient build --target <target>` and runs it. It is ES modules with no dependencies, and knows of no browser and no Node: a target for either builds on it.

```js
import { load } from "./host.js";

const program = await load(fetch("page.wasm"), {
  // By the ID in the package's `ambient.toml`, then the extern's module path and name.
  "<package id>": {
    "main.said": (line) => console.log(line),
    // One that gives a promise answers later: its extern is declared `suspending`.
    "main.slept": (ms) => new Promise((wake) => setTimeout(wake, ms, null)),
    // A function it is handed is one it calls, while it runs.
    "main.applied": (work, to) => work(to),
    // After its arguments comes the call, which performs the abilities of its row.
    "main.echoed": (line, call) =>
      call.perform("<package id>", "main.say", line),
  },
});
console.log(await program.run());
```

`examples/browser` is a page that does this.

A program built to be loaded into takes more code while it runs. No command builds one yet: `backend::wasm::open_module` and `unit` do.

```js
// The function a unit holds, which the host holds until it hands it over or lets it go.
const plugin = await program.add(fetch("plugin.wasm"), natives);
console.log(await program.call(plugin, 4));
plugin.release();
// A suspending extern that answers with one hands it to the program.
const fetched = () => program.add(fetch("plugin.wasm"));
```

## Testing

- `cargo nextest run -p ambient-js` builds a program and runs `js/*.test.js` under Node's own runner (`tests/host.rs`).
- The tests are thin: that the host runs what the compiler makes. What a program means is tested in `crates/ambient`.
- `constants.js` is what the runtime defines, written by `build.rs` when the crate is built and never committed. Build the crate before loading `js/host.js` from here.
- Nothing here is installed from a registry. Keep it so.

See "WebAssembly hosts" and "JavaScript hosts" in `spec/targets.md`.
