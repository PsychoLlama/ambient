# Definitions

A definition is the unit of naming, hashing, checking, compiling, and exchange. Every definition has an interface, which dependents check against, and a hash, which identifies its entire content.

## Kinds

- Types, functions, abilities, row aliases, externs, targets, [keys](methods.md), and tags are definitions.
- Functions are the only definitions with a body.
- A lambda or nested function is its own definition once checked: it is hashed separately and its encloser refers to it by hash.
- There are no top-level values; a constant is a function with no parameters.

## Interface

- A type's interface is the type itself.
- A function's interface is its signature: type parameters, parameter types, return type, and [row](abilities.md).
- An ability's or extern's interface is its signature and its identity.
- An interface is determined by the definition's header and the interfaces of the types it names, never by any body.
- Nothing in a header is inferred: an omitted return type is `null` and an omitted row is empty.
- A lambda's header is filled in from the context it is checked in; that is the only inference that produces an interface.
- A lambda parameter written without a type takes it from the function type expected of the lambda; its return type is the one it states, or else its body's.
- A lambda passed to a generic function is checked after the other arguments, which supply the type parameters its own parameters mention.
- A lambda may take [type parameters](types.md) of its own, in scope for everything written in it.
- A lambda written as a handler in a record takes its parameter types from the ability's signature.

## Hash

- A hash covers a definition's [core tree](trees.md) with every reference replaced by the hash of what it refers to.
- Structure, literals, and record field names are content, and so is the identity of a definition that has one.
- Names of definitions, parameters, locals, and type parameters are not content; neither are doc comments, source positions, declaration order, or the module a definition lives in.
- Renaming a definition that has no identity, or moving it within its package, never changes any hash.
- Two definitions with the same content have the same hash, whatever their [symbols](symbols.md).
- A function's hash covers its signature.
- Hashes are independent of backend and platform; no compiled artifact feeds a hash.
- A [comptime](comptime.md) result is content of the definition it was spliced into.
- An extern's hash covers only its identity and signature; see [externs](externs.md).

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

## Identity

- Some definitions are told apart by where they are declared, since structure cannot tell them apart: [brands](types.md), [keys](methods.md), [abilities](abilities.md), [externs](externs.md), and tags.
- The identity of one is its package's [ID](packages.md), its module path, and its name. Nothing else in the language is nominal.
- An identity is written nowhere: it is read from the declaration, so no second name can drift from the first or collide with another by accident.
- An identity is content. It is the one place a name reaches a hash.
- Renaming such a definition, or moving it to another module or package, makes it a different one, and changes the hash of everything that refers to it.
- Two packages that declare the same name in the same module path declare different things; a fork keeps its origin's ID, and so its origin's identities.
- A definition with an identity is reached like any other: imported, exported, and found by its [symbol](symbols.md). Nothing refers to one by spelling its identity.
- TODO: nothing maps an old identity to a new one, so a rename or a move parts values and programs made before it from those made after.

## Tags

- A tag is a definition with an identity and nothing else.
- Naming a tag where a value is expected gives a tag value: a comparable, serialisable token that is equal only to the same tag.
- Naming it where a type is expected gives the [literal type](types.md) of that value.
- A tag is declared before it is used: no expression makes one, and none is written by its text.
- Another package uses a tag only if the tag is exported to it.
