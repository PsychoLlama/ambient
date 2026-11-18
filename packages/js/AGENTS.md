# js

The JavaScript target: an Ambient package whose tool builds a program for a JavaScript host, and the host that loads and runs it. The root `AGENTS.md` holds what spans crates.

## Invariants

- `src/lib.ab` is the tool (`bundle`) and the target it builds (`host`), which binds nothing. A browser's target or Node's extends it; neither is here yet.
- A build is the module, `program.js`, the host under `runtime/`, and the host code of each module that declares an extern the program reaches, under `externs/<package id>/`. `program.js` imports a function for each of those externs by name and no other.
- An extern annotated with `binding` has no host code: `program.js` holds a function for it, written from the annotation and the extern's signature (`src/glue.ab`). One that gives a handle mints it under the brand its signature names.
- What a binding may say is listed once, on the tag `binding` in `src/lib.ab`. Bindings for a platform's own interfaces belong in a package for that platform, never here.
- `program.d.ts` is written from signatures (`src/types.ab`): what `run` takes and gives, and what each extern's JavaScript is handed. A type is written as `host/values.js` makes its values; change the two together.
- Only a path of names from a binding is written into a build as code.
- A package keeps its host code in a directory `js` its manifest lists: a file at each module's path that exports a function for each extern, by its name.
- `host/` is ES modules with no dependencies, which know of no browser and no Node. Install nothing from a registry.
- `constants.js` is written by the tool from what `build.compile` says the runtime defines, into the build. It is no file of the package; restate no constant in JavaScript.
- The host restates what `backend/wasm`'s host does; where they differ, that one is right.
- Native code calls back only through its `Call`, which is closed when it returns or gives a promise; a function it was handed is the same call by another name.
- `units.js` restates `backend/wasm/units.rs`. Loads go one at a time and nothing is reclaimed while one instantiates, since a unit is told addresses and how many externs the program has.
- A `Loaded` owns one reference to its function, and gives it up once: by being handed to the program, or by `release`.
- A run keeps its own waits, by its context (`Waits`), so runs at once never share an answer.

## Testing

- `crates/ambient-js` builds a program with this package and runs `js/*.test.js` there under Node's own runner. JavaScript is tested nowhere else, and only what the host and the loader themselves do.
