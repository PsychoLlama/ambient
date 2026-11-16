# Externs

An extern is a definition implemented in native code. Ambient declares the interface, and a native bundle supplies the implementation by the extern's [identity](definitions.md).

## Kinds

- An extern function is a pure native function; callers cannot tell it from an Ambient function.
- A native handler is a native implementation of an [ability](abilities.md).
- A native handler cannot be called; it can only be a root binding in a [target](targets.md).
- Effectful native code is therefore reachable only by performing an ability, never by knowing a hash.
- An extern function's purity is its author's contract and is not verified.

## Declaration

- An extern is declared in Ambient with a full signature, and is checked like any other header.
- A native handler names the ability it implements and takes that ability's signature.
- A native handler may declare a row of abilities it performs.
- A native handler that may suspend is declared `suspending`; one that is not never does.
- An extern with no native counterpart for the selected target is a link error only if the entry reaches it.

## Native side

- Native code can construct any [value shape](values.md), mint handles, and call function values it is given.
- Native code performs the abilities in its declared row with the same bindings Ambient code would get.
- Native code runs only when called; it has no initialisation and no background behaviour.
- Native code is handed its arguments and the bindings of its caller, and gives back a value or raises a [fault](values.md).
- Native code is also handed the program that called it, and can [load](backends.md) a function into it; what it gets back is that function as a value.
- A suspending native handler may suspend the [task](tasks.md) that performed it while it waits on the host.
- It suspends by saying it will answer later. Other tasks run, and the perform ends when the host gives the answer: a value, or a fault.
- Native code still runs only when called: whatever does the waiting belongs to the host, which hands the answer over when it next calls in.
- A task cancelled while it waits leaves the wait. The host is told it may stop, and an answer that comes anyway is dropped.
- A handle's type is a [brand](types.md) declared alongside the extern that mints it.

## Calling back

- Native code calls a function value only while it is itself called, and is given what the function gives.
- The call belongs to the [task](tasks.md) that called the native code, so nothing runs that a task of the program did not start.
- A function type in an extern's signature has an empty row. What is handed over was bound where it was handed, by the [capture](abilities.md) rule, so native code holds no bindings for a function that is not its own.
- Native code performs an ability of its declared row by calling the handler bound for it, as it calls any function value.
- A function native code called cannot suspend, since nothing can save the native code beneath it. One that would is a [fault](values.md).
- Code that waits is therefore written in Ambient, around native code that does not.
- A call back that unwinds, by a fault, an [abort](handlers.md), or a cancellation, unwinds the native code that made it: the native code lets go of what it holds and cannot answer.
- Native code that calls back counts toward the [depth of calls](backends.md#calls) at a fixed size, so calls nested through native code fault at the limit as any others do.
- TODO: nothing calls back yet: native code computes from its arguments, and besides loading reaches nothing of its program.

## Linking

- A program lists the externs its entry reaches; whoever loads it supplies native code for each, found by its identity, before it runs.
- Compiled code names an extern by its position in that list, never by address, so an [artifact](build.md) is the same whatever native code it is loaded with.
- A program that reaches an extern nobody supplied is not loaded.

## Distribution

- A package carries its native bundles precompiled, per platform, beside the extern declarations.
- Only externs the entry reaches are linked into a program.
- That a package contains native code is disclosed at install, alongside its [build grants](comptime.md).
- Definition hashes do not cover native code; a package's [snapshot](packages.md) does.
