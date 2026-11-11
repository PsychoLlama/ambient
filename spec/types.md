# Types

Types are structural, erased at runtime, and content-addressed. A type is checked and cached without reference to any function.

## Forms

- Primitives: `null`, `bool`, `int` (64-bit signed), `float` (64-bit), `string` (UTF-8), `bytes`.
- Literal types: a single `bool`, `int`, or `string` value.
- Records: a set of keys, each with a type; a key is a field name or an [ability](handlers.md).
- Tuples: a fixed sequence of types.
- Collections: `List<T>`, `Map<K, V>`, `Set<T>`.
- Functions: parameter types, a return type, a [row](abilities.md), and optional type parameters; parameter names are not part of the type.
- Unions and intersections.
- `never`, a subtype of every type, and `unknown`, a supertype of every type.
- Brands.
- Type parameters, each with an optional upper bound.

## Subtyping

- Subtyping is structural, and coinductive over recursive types.
- A record is a subtype of any record with fewer keys or wider key types.
- Collections are covariant; immutability makes that sound.
- A function type is a subtype of one with narrower parameters, a wider return type, or a larger row.
- A literal type is a subtype of its primitive.
- An intersection of records has the keys of both; an intersection with a brand adds the brand.
- An intersection of two different shapes is `never`; there are no overloaded function types and no negation types.
- `int` and `float` are unrelated.

## Unknown and casts

- `unknown` supports no operations.
- A cast tests a value against a type and yields either the value at that type or its alternative branch; it is the only way down from `unknown`.
- A cast decides by the value's [shape](values.md), so any erased type can be a cast target.
- A cast never yields a branded type.
- Values that enter from outside the program, [received values](exchange.md) included, enter as `unknown`.

## Narrowing

- Match is the only narrowing construct: its arms test literals, shapes, and types.
- A match is exhaustive over the static type of its subject.
- Exhaustiveness stays sound under [exchange](exchange.md): a union with another member is a different type with a different hash, and foreign values arrive as `unknown`.

## Brands

- A brand is a type with a tag and no structure.
- `T & Brand` is a subtype of `T`; going the other way is the branding operation.
- Branding is permitted only in the module that declares the brand.
- Brands are erased: they constrain code compiled from source and are not a security boundary; see [exchange](exchange.md).
- A brand on its own is the type of a [handle](values.md).

## Generics

- Type and function definitions take type parameters; a bound is a subtype bound.
- Type arguments are inferred at the use site or written explicitly.
- A function type may itself be generic.
- A recursive generic type applies itself only to its own parameters, unchanged.
- There are no conditional types, mapped types, or type operators.

## Type definitions

- A non-recursive type definition is transparent: it has the hash of its expansion, and naming it changes nothing.
- A recursive type definition is a member of a [group](definitions.md).
- Equal hashes imply equal types; unequal hashes imply nothing, since a recursive type has many unfoldings.
- Subtyping decides type relations; hashes only shortcut and cache it.

## Checking

- Checking is bidirectional: headers supply the types of definitions, and everything else is inferred within one definition.
- Local bindings, lambda parameters, and type arguments are inferred; nothing in a header is.
- A subtyping judgement between two closed types is a pure function of their hashes.
