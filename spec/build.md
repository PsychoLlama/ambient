# Build

Compilation is a set of demand-driven queries over content-addressed inputs. Nothing is computed until a root asks for it, and nothing is computed twice.

## Demand

- A build starts from a root: an entry and a [target](targets.md), or a tool's question about a symbol.
- Only definitions the root reaches are resolved, checked, or compiled.
- Checking a whole package is a root that names every definition in it.
- A program contains exactly the closure its entry reaches.
- Language primitives are not definitions and are always present.

## Stages

- Each stage is a pure function of its inputs, keyed by their content.
- Read: a module's source yields its definition table and [surface trees](trees.md).
- Resolve: a surface tree and the definition tables it imports from yield [symbols](symbols.md).
- Check: a resolved tree and the interfaces it refers to yield a core tree.
- Hash: a core tree and the hashes it refers to yield a hash.
- Compile: a core tree and a backend yield an artifact.
- What each edit invalidates is stated in [definitions](definitions.md).

## Store

- One store per workspace holds definitions by hash, stage results, the index, [memoized runs](memoization.md), and artifacts.
- Every entry is either content-addressed or derived from entries that are; deleting the store loses only time.
- An entry is valid on any branch, checkout, or machine, because no key contains a path or a time.
- The compiler and every tool read and write the same store.
- A long-lived server is an optimisation over the store, never a requirement.

## Backends

- The core tree is the only input to code generation.
- The backends are native code and WebAssembly.
- Artifacts are keyed by hash and backend and never feed a hash.
