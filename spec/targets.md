# Targets

A target defines the world a program runs in: a root binding for every ability that world offers. Compiling for a platform, embedding in a host, running a build, and running tests each amount to choosing a target.

## Root bindings

- A target is a definition that maps abilities to handlers; each handler is a [native handler](externs.md) or an Ambient function.
- A program is an entry function and a target.
- A program runs as its entry under an only-mode [provide](handlers.md) of its target.
- The entry's row must be covered by the target.
- A root handler's own row is satisfied by the target's other bindings; a cycle among them is an error.
- Root handlers are therefore provided in layers, each over the handlers its rows need, and only the outermost is only-mode.
- A root handler for an ability that does not return ends the program, and its result is the program's.
- An entry has no type parameters; its parameters are the program's.
- No ability has an implementation outside a provide; a target is only the outermost one.
- A target also fixes how [tasks](tasks.md) are scheduled.

## Composition

- A target may extend other targets, adding and replacing bindings; a conflict between extended targets is resolved explicitly.
- Libraries export abilities and handlers for them; targets select among them.
- A program runs unchanged on any target that covers its row.
- Portability is a property of rows, not of conditional compilation.

## Pruning

- A root binding the entry's row does not reach is neither compiled nor linked.

## Embedding

- An embedder defines a target from the host language and supplies native handlers directly.
- An embedder's target need not include any standard binding; a program is then limited to pure code and what the embedder provides.

## Standard targets

- The standard library defines a target for a program run from a command line: its streams, its arguments and environment, files, clocks, and its exit.
- Each of those is a module of abilities, so a row says which of them a function reaches.
- An operation on a file that can fail in ordinary use gives an [error](errors.md) as a value; the kinds are tags of the module.
- Exiting is an ability that does not return, so the program's result is the code it exits with.
- TODO: there is no standard target for a WebAssembly host yet.
- The build target and test targets are ordinary targets; see [comptime](comptime.md) and [memoization](memoization.md).
