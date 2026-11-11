# Abilities

An ability is an operation that code may perform but does not implement. Abilities are the only way a function reaches anything beyond its arguments.

## Definition

- An ability is a tag and a signature; it has no body and no default implementation.
- An ability's identity is its tag and signature together; an implementation can change without the ability changing, because the ability has none.
- Abilities are standalone: no enclosing interface or set owns one.
- Abilities are static: they are defined in modules and never created at runtime.
- An ability is not a value; it appears only in rows, in performs, and as a [bundle](handlers.md) key.
- An ability's signature may be generic, but an ability takes no type parameters of its own.

## Rows

- A row is a set of abilities; it has no order and no duplicates.
- A function's row is exactly what it takes from its caller.
- A row alias names a row and expands wherever it is used; it has no identity of its own.
- A definition with an empty row reaches nothing but its arguments; see [determinism](cells.md).
- A function value with an empty row may still act through handlers it captured.

## Performing

- A perform names an ability statically, passes arguments, and evaluates to the handler's result.
- Every call passes the callee a handler for each ability in its row, taken from the caller's own bindings.
- Bindings are only ever passed; there is no table to look an ability up in.
- A function cannot perform an ability it was not passed, whatever it knows about that ability.
- A perform of an ability whose signature returns `never` does not return; see [abort](handlers.md).

## Capture

- A lambda's row lists what it takes from its caller; every other ability it performs is captured.
- A captured ability is bound, when the function value is created, to the handler in scope there.
- A captured handler travels with the value and is used wherever the value is called, including under a scope that rebinds or removes the ability.
- A function value whose row exceeds the row expected of it is bound in place: the excess is captured from the current scope.
- Binding an ability that is not in scope is a type error.
- Higher-order functions therefore declare no abilities on behalf of their callbacks, and the language has no ability or row parameters.
- A top-level function captures nothing.
- Capture is how authority is delegated: whoever holds a function value can exercise what it captured.
- Captures are explicit in the [core tree](trees.md) and statically known.

## Errors

- `throw` is an ordinary ability that returns `never`; it is implicit in no row.
- Rows do not lie about aborts: an abort raised by a handler is charged to the scope that bound the handler, not to the code that performed.
