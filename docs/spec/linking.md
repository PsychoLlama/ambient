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
- What a program holds of a [definition](definitions.md) beside its code is data. Laying a program out is one piece of code that reads it, however many definitions there are.
- A piece a program carries is part of the program: calling one is a call like any other, never a call out to a runtime loaded beside it.
- Where the checker knows a value's type it picks the operation for that type. Only code that does not know reaches a piece that handles every shape.
- A program that performs no [task](tasks.md) operation carries no scheduler.
- A program says when it is compiled whether code can be [loaded](backends.md) into it. One that cannot is known whole, so what it pays for is decided over all of it.
- Natively no function is compiled to suspend or to resume: a task there is a [stack](backends.md#suspension). The rest of this list is what a WebAssembly program pays.
- A program nothing is loaded into, in which no [task](tasks.md) can suspend, has no function compiled to suspend or to resume.
- A program code can be loaded into has every function compiled to suspend, since what is loaded may suspend in the middle of any call. Loading into any other program is refused.
- In a program that can suspend, a function that makes no call is still compiled without it: nothing can suspend while it runs.
- A [native handler](externs.md) says whether it suspends, so a program pays to suspend for the externs it reaches only if one of them does.
- TODO: one task operation anywhere makes every function that calls another pay to suspend, though most are never running when a task does.

## Hosts

- A host reaches a program through pieces too: the ones that start a call, and the ones that make and read [values](values.md).
- A program is emitted for the pieces its host says it calls, so a host that reads no values pays for no readers.

## Loading

- Code emitted to be [loaded](backends.md) is told what its target has: a set of hashes, of definitions and of pieces. It carries the rest and refers to what it was told of.
- It is told when it is emitted, not when it is loaded. Being told of something the target lacks fails the load; being told of less only makes the code larger.
- Told of nothing, the code loads into any program.
- State is the exception: loaded code uses the target's, by hash, and brings its own only if the target has none.
- What loaded code brings of the runtime is its target's from then on, and outlives the code: a value may refer into a piece long after the definitions that came with it are gone.
- Later code is told of those pieces like any others, so a piece is brought twice only by code that was told of less than its target had.
- What to load together, and when, is not the language's to decide. A bundler, a plugin host, and a live upgrade are libraries over the same two questions: what a definition reaches, and what a target has.
- Loaded code and its target must agree on the layout of every value. One number names that agreement; a load whose number differs is refused.
- Comparing that number before sending code is up to whoever sends it. The language makes no promise across it.
- Adding a piece does not change the number, so a program that is running can load code that uses operations newer than itself.

## Native bundles

- A [native bundle](externs.md#native-bundles) is linked as a piece is: it refers to the pieces of the runtime it calls, and a program carries a function of it only if the program reaches that function's extern.
- A native bundle and the runtime it is linked with must agree on the layout of every value, by the number [loaded code](#loading) is held to. A native bundle made for another number is not linked.
- A change of that number therefore asks every native bundle to be made again, and nothing more: no native bundle is kept working across it.
- A native bundle shares names with the rest of a program only where it must: the functions of its externs, and what it calls of the runtime. Whatever else it holds is its own.
- So a native bundle may carry the library of the language it was written in, though the host and another native bundle carry the same one. Each keeps its copy, and a program carries only what its functions reach of it.
- A native bundle that defines a part of the runtime is not linked: it would use its own in place of its program's.
- What a package's native code may do to a value is what the runtime's own code may, and nothing checks either.

## Native code

- A native executable is linked from what its program reaches of its host: the pieces of the runtime, the native side of each ability its [target](targets.md) binds, and the functions of native bundles.
- The linker is told what the program reaches and never sees its code. The program is carried as it was compiled, so linking changes no [artifact](build.md).
- Machine code is linked by the linker of the machine that builds, whether or not the program reaches a native bundle. A machine with none builds no native executable, and says so.
- One way makes every native executable, so what a program is given never depends on what the machine that built it had.
- Loaded code brings no machine code of the runtime, so a native program code can be loaded into holds every piece.
- TODO: a native executable holds every piece of the runtime, since its program is carried as data the linker never sees.
