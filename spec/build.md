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
- Check: a resolved tree and the interfaces it refers to yield an [open core definition](core.md).
- Hash: an open definition and the hashes of its slots yield a hash and a closed definition.
- Compile: a core tree, the interfaces of its slots, and a backend yield an artifact.
- What each edit invalidates is stated in [definitions](definitions.md).

## Store

- One store per workspace holds definitions by hash, stage results, the index, [memoized runs](memoization.md), artifacts, and the files an [annotation](annotations.md) or a [tool](tools.md) names by digest.
- A stage result is kept under a key derived from the content of that stage's inputs, so an unchanged input is never recomputed.
- The compiler is an input to every stage: a result one build of it stored is never reused by another.
- Every entry is either content-addressed or derived from entries that are; deleting the store loses only time.
- An entry is valid on any branch, checkout, or machine, because no key contains a path or a time.
- The compiler and every tool read and write the same store.
- A long-lived server is an optimisation over the store, never a requirement.

## Backends

- The core tree is the only input to code generation.
- The [backends](backends.md) are native code and WebAssembly.
- An artifact is keyed by what compiling read, never by its definition's hash, and never feeds a hash.
