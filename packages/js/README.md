# The JavaScript target

An Ambient package that builds a program for a JavaScript host. A target that extends `host` is built by this package's tool:

```
import js.{host};

pub ability say(text: string): null;
extern handler said for say;

pub target page extends host { say: said }
```

The package that declares an extern keeps its JavaScript in a directory `js`, which its manifest lists (`files = ["js"]`): a file for each module, which exports a function for each extern by its name.

```js
// js/main.js, beside src/main.ab
export const said = (line) => console.log(line);
// One that gives a promise answers later: its extern is declared `suspending`.
export const slept = (ms) => new Promise((wake) => setTimeout(wake, ms, null));
// A function it is handed is one it calls, while it runs.
export const applied = (work, to) => work(to);
// After its arguments comes the call, which performs the abilities of its row.
export const echoed = (line, call) =>
  call.perform("<package id>", "main.say", line);
```

An extern whose native side is a function JavaScript already has needs no file. It is annotated with the path of that function from the global object, and the build writes the call:

```
import js.{binding};

@binding("Math.max") extern fn larger(one: int, other: int): int;
```

`ambient build <package> --target main.page` writes a directory:

- `program.wasm`: the program.
- `program.js`: what loads it, with a function for each extern the program reaches and no other.
- `runtime/`: the host (`host/` here), and the constants the compiler that made the module says it must agree with.
- `externs/`: the JavaScript of each module that declares an extern the program reaches.

```js
import { start } from "./build/program.js";

const program = await start();
console.log(await program.run());
```

`start` fetches the module from beside `program.js`; hand it the bytes, or a response that carries them, where there is no `fetch` of a file. Its second argument is native code to use in place of the program's own, as `{ "<package id>": { "main.said": ... } }`.

`examples/browser` is a page that does this.

A program built to be loaded into takes more code while it runs. No command builds one yet: `backend::wasm::open_module` and `unit` do.

```js
// The function a unit holds, which the host holds until it hands it over or lets it go.
const plugin = await program.add(fetch("plugin.wasm"), natives);
console.log(await program.call(plugin, 4));
plugin.release();
```

## Testing

- `cargo nextest run -p ambient-js` builds a program with this package and runs `crates/ambient-js/js/*.test.js` under Node's own runner.
- Nothing here is installed from a registry. Keep it so.

See "JavaScript hosts" in `spec/targets.md` and `spec/tools.md`.
