# Abilities

An ability is an operation that code may perform but does not implement. Abilities are the only way a function reaches anything beyond its arguments.

## Definition

- An ability is a signature with an [identity](definitions.md); it has no body and no default implementation.
- Two abilities are the same only if identity and signature both agree; an implementation can change without the ability changing, because the ability has none.
- Abilities are standalone: no enclosing interface or set owns one.
- Abilities are static: they are defined in modules and never created at runtime.
- An ability is not a value; it appears only in rows, in performs, and as a [bundle](handlers.md) key.
- An ability's signature may take [type parameters](types.md); a perform gives them by its arguments, as a call of a generic function does.
- They are the signature's, chosen at each perform: an ability is one ability, bound once, whatever types it is performed at.
- A handler for such an ability is a generic function with the same type parameters.
- An ability that does not return takes no type parameters: nothing comes back for them to relate to what was passed.

## Rows

- A row is a set of abilities; it has no order and no duplicates.
- A function's row is exactly what it takes from its caller.
- A module named in a row stands for every public ability of it, and expands wherever it is used: a row is only ever its abilities.
- There is no other way to name a row. Abilities that belong together are a module, which is already how they are declared.
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
- A lambda that states no row, written where a function type is expected, takes that type's row: a callback reaches what whoever calls it provides.
- A captured ability is bound, when the function value is created, to the handler in scope there.
- A captured handler travels with the value and is used wherever the value is called, including under a scope that rebinds or removes the ability.
- A function value whose row exceeds the row expected of it is bound in place: the excess is captured from the current scope.
- A list, a tuple, or a record written where one is expected has each part held to what is expected of that part, so a function written in one is bound in place too. A value made elsewhere is never rebuilt to bind what it holds.
- Where a union is expected, such a value is held to the one member it could be: the list, the record, or the tuple of its length. With no such member, or more than one, nothing is expected of its parts.
- Binding an ability that is not in scope is a type error.
- Higher-order functions therefore declare no abilities on behalf of their callbacks, and the language has no ability or row parameters.
- A top-level function captures nothing.
- Capture is how authority is delegated: whoever holds a function value can exercise what it captured.
- Captures are explicit in the [core tree](trees.md) and statically known.

## Errors

- `throw` is an ordinary ability that returns `never`; it is implicit in no row and carries only an [error](errors.md).
- Rows do not lie about aborts: an abort raised by a handler is charged to the scope that bound the handler, not to the code that performed.
