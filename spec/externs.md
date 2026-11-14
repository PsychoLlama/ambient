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
- An extern with no native counterpart for the selected target is a link error only if the entry reaches it.

## Native side

- Native code can construct any [value shape](values.md), mint handles, and call function values it is given.
- Native code performs the abilities in its declared row with the same bindings Ambient code would get.
- Native code runs only when called; it has no initialisation and no background behaviour.
- Native code is handed its arguments and the bindings of its caller, and gives back a value or raises a [fault](values.md).
- Native code is also handed the program that called it, and can [load](backends.md) a function into it; what it gets back is that function as a value.
- TODO: native code cannot yet call a function value or perform an ability; besides loading, it can only compute from its arguments.
- A native handler may suspend the [task](tasks.md) that performed it while it waits on the host.
- A handle's type is a [brand](types.md) declared alongside the extern that mints it.

## Linking

- A program lists the externs its entry reaches; whoever loads it supplies native code for each, found by its identity, before it runs.
- Compiled code names an extern by its position in that list, never by address, so an [artifact](build.md) is the same whatever native code it is loaded with.
- A program that reaches an extern nobody supplied is not loaded.

## Distribution

- A package carries its native bundles precompiled, per platform, beside the extern declarations.
- Only externs the entry reaches are linked into a program.
- That a package contains native code is disclosed at install, alongside its [build grants](comptime.md).
- Definition hashes do not cover native code; a package's [snapshot](packages.md) does.
