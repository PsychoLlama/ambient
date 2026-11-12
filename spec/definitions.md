# Definitions

A definition is the unit of naming, hashing, checking, compiling, and exchange. Every definition has an interface, which dependents check against, and a hash, which identifies its entire content.

## Kinds

- Types, functions, abilities, row aliases, externs, and targets are definitions.
- Functions are the only definitions with a body.
- A lambda or nested function is its own definition once checked: it is hashed separately and its encloser refers to it by hash.
- There are no top-level values; a constant is a function with no parameters.

## Interface

- A type's interface is the type itself.
- A function's interface is its signature: type parameters, parameter types, return type, and [row](abilities.md).
- An ability's or extern's interface is its signature and its tag.
- An interface is determined by the definition's header and the interfaces of the types it names, never by any body.
- Nothing in a header is inferred: an omitted return type is `null` and an omitted row is empty.
- A lambda's header is filled in from the context it is checked in; that is the only inference that produces an interface.
- A lambda parameter written without a type takes it from the function type expected of the lambda; its return type is always its body's.
- A lambda passed to a generic function is checked after the other arguments, which supply the type parameters its own parameters mention.
- A lambda written as a handler in a record takes its parameter types from the ability's signature.

## Hash

- A hash covers a definition's [core tree](trees.md) with every reference replaced by the hash of what it refers to.
- Structure, literals, record field names, and tags are content.
- Names of definitions, parameters, locals, and type parameters are not content; neither are doc comments, source positions, declaration order, or the module a definition lives in.
- Renaming a definition, or moving it within its package, never changes any hash.
- Two definitions with the same content have the same hash, whatever their [symbols](symbols.md).
- A function's hash covers its signature.
- Hashes are independent of backend and platform; no compiled artifact feeds a hash.
- A [comptime](comptime.md) result is content of the definition it was spliced into.
- An extern's hash covers only its tag and signature; see [externs](externs.md).

## Groups

- Definitions that refer to each other cyclically form a group, hashed as one unit; a member is identified by the group hash and its index.
- Member order is derived from content, never from names or source order.
- Types and functions never share a group, because a type cannot refer to a function.
- A definition outside any cycle is hashed alone.

## Invalidation

- Checking a definition depends on its own tree and the interfaces of the definitions it refers to, and on nothing else.
- Editing a body rechecks and recompiles that definition alone.
- Dependents of an edited body receive new hashes without being rechecked or recompiled; compiled code refers to dependencies indirectly so that this holds.
- Editing a function's header rechecks its direct dependents and stops there, because their interfaces do not depend on it.
- Editing a type propagates through every interface that names it, and no further.

## Tags

- A tag is a string literal that supplies identity where structure cannot: on [brands](types.md), [abilities](abilities.md), and [externs](externs.md).
- A tag may also be written as a value: a comparable, serialisable token that is equal only to the same tag.
- Nothing else in the language is nominal.
- A tag is unique within the package that declares it and is scoped by that package's [ID](packages.md), never by a name.
- Equal strings in two packages are different tags; a fork keeps its origin's tags.
- A tag is content: editing it, or moving its definition to another package, changes the hash.
