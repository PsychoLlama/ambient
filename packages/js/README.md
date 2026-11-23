# The JavaScript target

An Ambient package that builds a program for a JavaScript host. A program under `host`, or a target that extends it, is built by this package's tool. A native handler says it answers there:

```
use js.{host};

pub ability say(text: string): null;
extern handler say in host;
```

The package that declares an extern keeps its JavaScript in a directory `js`, which its manifest lists (`files = ["js"]`): a file for each module, which exports a function for each extern by its name.

```js
// js/main.js, beside src/main.ab
export const say = (line) => console.log(line);
// One that gives a promise answers later: its extern is declared `handler(suspending)`.
export const sleep = (ms) => new Promise((wake) => setTimeout(wake, ms, null));
// A function it is handed is one it calls, while it runs.
export const applied = (work, to) => work(to);
// After its arguments comes the call, which performs the abilities of its row.
export const echoed = (line, call) =>
  call.perform("<package id>", "main.say", line);
```

A handler whose native side is something JavaScript already has needs no file. It is annotated with what it is, and the build writes its function:

```
use js.{binding, host};

pub ability larger(one: int, other: int): int;
#[binding("Math.max")]
extern handler larger in host;

type(nominal) Address;
pub ability address(text: string): Address;
pub ability host_of(address: Address): string;
#[binding({ new: "URL" })]
extern handler address in host;
#[binding({ property: "hostname" })]
extern handler host_of in host;
```

A path is a call of a global function; `get` reads one; `new` makes an object; `method`, `property`, and `set` are of the first thing the ability takes; `{ build: "digest" }` is the digest of the program's own module. An extern that gives a handle, as `address` does, gives the JavaScript object, which the host holds until the program lets the handle go.

`ambient build`, of a binary whose manifest names the target (`target = "js.host"` under `[bin.main]`), writes a directory, `build/<name>` for the binary's name:

- `program.wasm`: the program.
- `program.js`: what loads it, with a function for each extern the program reaches and no other.
- `program.d.ts`: its types for TypeScript: what `run` takes and gives, and what the JavaScript of each extern is handed.
- `runtime/`: the host (`host/` here), and the constants the compiler that made the module says it must agree with.
- `externs/`: the JavaScript of each module that declares an extern the program reaches.

```js
import { start } from "./build/<name>/program.js";

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

See "JavaScript hosts" in `docs/spec/targets.md` and `docs/spec/tools.md`.
