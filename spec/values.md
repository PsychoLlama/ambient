# Values

Values are immutable and self-describing. A value's runtime shape is enough to test it against any type, with no static information about where it came from.

## Shapes

- The shapes are null, bool, int, float, string, bytes, list, map, set, tag, record, tuple, function, cell, [iterator](iteration.md), and handle.
- No shape is user-defined; every [type](types.md) describes a subset of these.
- A function value is a definition hash, captured values, [captured handlers](abilities.md), and the type arguments it was made at if its function [needs them](backends.md).
- A function value says which type it is of, as one name, and nothing of what that type is within.
- A function whose type depends on type arguments is of that type at the arguments it holds, and says so.
- TODO: a function holds type arguments only if it [needs them](backends.md) as it runs. One whose type depends on arguments it does not hold says no type, so it never comes back out of `unknown`.
- A handle is an opaque reference minted by [native code](externs.md).
- A handle carries the [brand](types.md) it was minted under and has no structure to inspect.
- No cast creates a handle.
- A handle is the brand and a number its host knows it by; two handles are the same when both are.
- A host is told when a program holds a handle no longer, so that what it stood for can be closed. It is told before native code next runs, or when no task can run, or when the program is over, whichever is first.
- What a program still holds when it is over goes with it, and its host is not told of each.

## Sharing

- Values are shared freely, between tasks included; sharing never copies.
- An update yields a new value that shares structure with the old one.
- A list shares its members with the lists made from it: a [push or a join](operations.md) onto the newest of them adds only what is new, and a slice copies nothing.
- Programs may rely on this for cost: building a list a member at a time takes time in proportion to its length.
- It does not depend on who else holds the list. Pushing twice onto one list copies it the second time, since the first push took the room after it.
- TODO: a slice keeps every member of the list it was cut from alive, and a list keeps what was pushed onto it and since dropped.

## Serialization

- Every value that is not anchored has one canonical encoding, determined by its shape alone.
- Equal values encode identically, so the encoding gives every value a hash.
- A function value encodes as its definition hash and its captures; the definition itself travels as a [pack](exchange.md).
- The bindings a function captured are among its captures, so a captured abort handler anchors it.
- TODO: type arguments have no encoding yet, so a function value that holds them cannot be sent.
- A decoded value is `unknown`.
- An [iterator](iteration.md) encodes as what steps it: a function, or what the language walks for it and how far it has got.
- A value is anchored if it contains a handle, a [cell](cells.md), a [native handler](externs.md), or an [abort](handlers.md) handler.
- An anchored value is tied to the program and scope that hold it and has no encoding.
- Anchoring is a property of the value, not a marker in its type; encoding an anchored value fails as an ordinary result, not a fault.
- A closed type with no function type, handle type, cell type, or `unknown` describes only unanchored values, so encoding a value of such a type cannot fail and is typed accordingly.

## Structural operations

- Equality, ordering, and hashing are defined by the language over every shape and cannot be extended.
- Function values compare by hash, captures, and the type arguments they hold; handles compare by identity.
- [Operations](operations.md) are defined per shape and cannot be overloaded.

## Numbers

- Integer arithmetic that leaves the range of `int` is a [fault](#faults), and so is dividing an integer by zero; nothing wraps.
- Float arithmetic follows IEEE 754 and never faults.

## Faults

- A fault is a violated runtime invariant.
- A fault is not an ability: no handler observes it and no row mentions it.
- A fault unwinds, running [cleanups](cleanup.md), to the nearest boundary; a program's root is a boundary.
- A boundary expression evaluates a body and yields either its value or the fault that ended it, as an [error](errors.md).
- A boundary cannot resume what faulted.
- A boundary contains only faults: an [abort](handlers.md) and a [cancellation](tasks.md) pass through it.
- Failures a program is expected to handle are union members or errors, never faults.
- Faults are for conditions no type can rule out, such as exhausted memory; an operation that can fail in ordinary use says so in its type.
