# ambient-native

What a native bundle is built against. The root `AGENTS.md` holds what spans crates; paths here are under `src/`.

## Invariants

- No dependencies, and `no_std`: a bundle links the runtime of the program it joins, never one of its own.
- `entries!` (`entries.rs`) is the one list of what a bundle may call. Add an entry there; the Rust declarations, `ENTRIES`, the C header, and the check in `ambient-host` all expand it.
- An entry's signature names only `Word`, `Call`, integers, and pointers to them. `ambient`'s header writer maps exactly those to C.
- Each entry's doc says who owns what it takes and gives. It is the only documentation a bundle's author has.
- `layout!` is the layout number, written once. A bundle defines `amb_built_<package id>_<layout>`, which the `main` of a program that reaches it refers to, so one built for another layout does not link.
- A constant here that restates the runtime's (`NULL`, `kind`) is held equal to it by a test in `ambient-host`.
