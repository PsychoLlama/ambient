# Values

Values are immutable and self-describing. A value's runtime shape is enough to test it against any type, with no static information about where it came from.

## Shapes

- The shapes are null, bool, int, float, string, bytes, list, map, set, tag, record, tuple, function, cell, and handle.
- No shape is user-defined; every [type](types.md) describes a subset of these.
- A function value is a definition hash, captured values, and [captured handlers](abilities.md).
- A function value carries its signature, so a cast to a function type is decidable.
- A handle is an opaque reference minted by [native code](externs.md).
- A handle carries the tag it was minted under and has no structure to inspect.
- No cast creates a handle.

## Sharing

- Values are shared freely, between tasks included; sharing never copies.
- An update yields a new value that shares structure with the old one.
- An update to a value nothing else holds happens in place; programs may rely on this for cost.

## Serialization

- Every value that is not anchored has one canonical encoding, determined by its shape alone.
- Equal values encode identically, so the encoding gives every value a hash.
- A function value encodes as its definition hash and its captures; the definition itself travels as a [pack](exchange.md).
- A decoded value is `unknown`.
- A value is anchored if it contains a handle, a [cell](cells.md), a [native handler](externs.md), or an [abort](handlers.md) handler.
- An anchored value is tied to the program and scope that hold it and has no encoding.
- Anchoring is a property of the value, not a marker in its type; encoding an anchored value fails as an ordinary result, not a fault.
- A closed type with no function type, handle type, cell type, or `unknown` describes only unanchored values, so encoding a value of such a type cannot fail and is typed accordingly.

## Structural operations

- Equality, ordering, and hashing are defined by the language over every shape and cannot be extended.
- Function values compare by hash and captures; handles compare by identity.
- Operators are defined per primitive and cannot be overloaded.

## Numbers

- Integer addition, subtraction, and multiplication that leave the range of `int` are [faults](#faults); nothing wraps.
- Float arithmetic follows IEEE 754 and never faults.

## Faults

- A fault is a violated runtime invariant.
- A fault is not an ability: no handler observes it and no row mentions it.
- A fault unwinds, running [cleanups](cleanup.md), to the nearest boundary; a program's root is a boundary.
- A boundary expression evaluates a body and yields either its value or the fault that ended it, as an [error](errors.md).
- A boundary cannot resume what faulted.
- Failures a program is expected to handle are union members or errors, never faults.
- Faults are for conditions no type can rule out, such as exhausted memory; an operation that can fail in ordinary use says so in its type.
