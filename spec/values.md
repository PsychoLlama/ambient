# Values

Values are immutable and self-describing. A value's runtime shape is enough to test it against any type, with no static information about where it came from.

## Shapes

- The shapes are null, bool, int, float, string, bytes, list, map, set, record, tuple, function, and handle.
- No shape is user-defined; every [type](types.md) describes a subset of these.
- A function value is a definition hash, captured values, and [captured handlers](abilities.md).
- A function value carries its signature, so a cast to a function type is decidable.
- A handle is an opaque reference minted by [native code](externs.md).
- No language operation or cast creates a handle, and a handle has no structure to inspect.

## Structural operations

- Equality, ordering, and hashing are defined by the language over every shape and cannot be extended.
- Function values compare by hash and captures; handles compare by identity.
- Operators are defined per primitive and cannot be overloaded.

## Faults

- A fault is a violated runtime invariant; it ends the running program.
- A fault is not an ability: no handler observes it and no row mentions it.
- Failures a program is expected to handle are union members or [abilities](abilities.md), never faults.
