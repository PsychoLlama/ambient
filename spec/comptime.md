# Compile Time

A comptime call runs an ordinary function during compilation and splices its result into the program. It is the single mechanism behind build scripts, code generation, and derived types.

## Calls

- A comptime call is marked at the call site; the callee is any function.
- There is no separate class of compile-time functions.
- An argument is an expression of [singleton type](types.md) or a closed type.
- The result replaces the call before checking; the enclosing definition's hash covers the result, not the call.
- A comptime call runs only when its enclosing definition is demanded by a [build](build.md).

## Splice positions

- In expression position the result is an [unanchored](values.md) value, spliced as a literal of singleton type.
- In type position the result is a type tree.
- In module position the result is a set of definitions, forming the whole content of a generated module.
- A generated module is the only place a comptime call creates [symbols](symbols.md); they are ordinary symbols.

## Trees as values

- Comptime functions receive and return types and definitions as [trees](trees.md), built with the same builders as everything else.
- Derived types such as a subset of a record's fields or a function's return type are ordinary functions over type trees.
- A comptime call operates on closed types only; a type parameter is never an argument.
- Type-level computation therefore never runs inside generic checking.
- A spliced tree is checked exactly as a written one; comptime cannot bypass the checker.
- A tree value carries no [metadata](trees.md); a comptime function reads names and doc comments by performing a verifiable build ability, so the read is in its [trace](memoization.md).

## Authority

- A comptime function's effects are abilities, so its row states everything it can touch.
- The compiler runs comptime calls under a build [target](targets.md) whose abilities read from the developer's machine.
- A [package](packages.md) declares the build abilities it needs; granting them is the installer's decision.
- A comptime call runs under the grants of the package that contains the call site.
- An ungranted ability is a compile error at the call, naming the ability.
- A comptime call with an empty row needs no grant.

## Caching

- Comptime calls are [memoized runs](memoization.md); a call reruns only when something it observed changed.
