# Linking

A program carries the code it reaches and nothing else, the [runtime](backends.md)'s included. What a feature costs is paid by the programs that use it.

## Pieces

- The runtime is a set of pieces: a function, or a block of data, each with the pieces it refers to.
- Compiled code refers to a piece as it refers to a [definition](definitions.md), so what a definition reaches is definitions and pieces alike.
- A program carries a piece only if something it carries refers to it.
- Adding an operation to the runtime therefore adds nothing to a program that does not use it.
- What a piece refers to is read from the piece, never declared beside it, so a program cannot lack a piece it reaches.
- A piece is named by a hash of its content and of the pieces it refers to. Pieces that refer to each other are hashed together.
- A piece of data the runtime writes to is state. A program has each state once, however many times code that refers to it was loaded.

## Floor

- Every program carries what calling, counting, and allocating need. That is the fixed cost of the language, and it is kept small.
- The floor does not grow with the runtime: a release that adds operations leaves it as it was.
- An operation a backend can express in a few instructions is emitted in place, and reaches a piece only for the cases that are not.
- Where the checker knows a value's type it picks the operation for that type. Only code that does not know reaches a piece that handles every shape.
- A program that performs no [task](tasks.md) operation carries no scheduler.
- TODO: every function still carries the code to suspend and to unwind, whether or not anything it reaches can.

## Hosts

- A host reaches a program through pieces too: the ones that start a call, and the ones that make and read [values](values.md).
- A program is emitted for the pieces its host says it calls, so a host that reads no values pays for no readers.

## Loading

- Code emitted to be [loaded](backends.md) is told what its target has: a set of hashes, of definitions and of pieces. It carries the rest and refers to what it was told of.
- It is told when it is emitted, not when it is loaded. Being told of something the target lacks fails the load; being told of less only makes the code larger.
- Told of nothing, the code loads into any program.
- State is the exception: loaded code uses the target's, by hash, and brings its own only if the target has none.
- What to load together, and when, is not the language's to decide. A bundler, a plugin host, and a live upgrade are libraries over the same two questions: what a definition reaches, and what a target has.
- Loaded code and its target must agree on the layout of every value. One number names that agreement; a load whose number differs is refused.
- Comparing that number before sending code is up to whoever sends it. The language makes no promise across it.
- Adding a piece does not change the number, so a program that is running can load code that uses operations newer than itself.

## Native code

- Compiled natively, a program runs in a process that holds the whole runtime, so its target has every piece.
