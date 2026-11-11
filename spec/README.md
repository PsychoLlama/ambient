# Ambient Spec

Ambient is a content-addressed language with structural types and abilities, built for programs that exchange code between machines and change while they run. This spec describes a from-scratch redesign; nothing here assumes the implementation documented under `ref/`.

## Goals

- Every definition is identified by a hash of its content; names are labels over hashes.
- An edit invalidates the least work that is sound, and that amount is predictable from the edit.
- The type system is sound; no construct trades safety for convenience.
- Effects are abilities: visible in signatures, provided explicitly, absent by default.
- Received code is safe to run: it can reach only what its receiver hands it.
- A program contains only what its entry reaches, native code included.
- Tooling is queries over a cache; no long-lived process is required for any of it.
- The language is extended with native code through packages, never by modifying the core.

## Language properties

- Values are immutable.
- A call in tail position never grows the stack.
- Modules contain only definitions; importing a module runs nothing.
- There is no ad-hoc polymorphism: no traits, overloads, operator overloading, or implicit conversion.
- There are no classes and no inheritance.

## Non-goals

- Text syntax. Notation in these pages is illustrative; see [trees](trees.md).
- First-class continuations. Handlers return or abort; see [handlers](handlers.md).
- Inference across definitions. See [definitions](definitions.md).
- Type-level computation. Derived types are computed by [comptime](comptime.md) calls.
- Ability and row parameters. Capture replaces them; see [abilities](abilities.md).

## Pages

- [definitions](definitions.md): the unit of hashing, checking, and caching.
- [symbols](symbols.md): canonical names, aliases, and the index.
- [types](types.md): structural types, subtyping, brands, generics.
- [values](values.md): runtime shapes, serialization, handles, faults.
- [abilities](abilities.md): abilities, rows, performing, capture.
- [handlers](handlers.md): provides, bundles, abort, sandboxing, recording.
- [comptime](comptime.md): compile-time calls and splices.
- [memoization](memoization.md): trace-validated caching for comptime and tests.
- [externs](externs.md): native functions and native handlers.
- [targets](targets.md): root bindings, platforms, embedding.
- [build](build.md): demand-driven stages, the store, backends.
- [trees](trees.md): surface and core trees, builders, metadata, text, formatting, edits.
- [modules](modules.md): modules, visibility, preludes.
- [packages](packages.md): package identity, publishers, dependencies, workspaces.
- [exchange](exchange.md): moving definitions and values between programs.

## Open

- TODO: live upgrade (generations, runtime-owned state, late-bound names) is not yet restated on this design.
- TODO: concurrency has no model; nothing here defines a task, and [externs](externs.md) and [handlers](handlers.md) both lean on one.
- TODO: mutable state has no model; handlers that accumulate cannot be written without it.
