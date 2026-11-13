# Types

Types are structural, erased at runtime, and content-addressed. A type is checked and cached without reference to any function.

## Forms

- Primitives: `null`, `bool`, `int` (64-bit signed), `float` (64-bit), `string` (UTF-8), `bytes`.
- Literal types: a single `bool`, `int`, `float`, `string`, or `bytes` value, or a single [tag](definitions.md).
- `Tag`, the type of every tag value.
- Records: a set of keys, each with a type; a key is a field name, a declared [key](methods.md), or an [ability](handlers.md).
- A record's key may hold a [method](methods.md), whose type also says what it needs of its receiver.
- Tuples: a fixed sequence of types.
- Collections: `List<T>`, `Set<T>`, and `Map<E>`, where `E` is the type of the map's entries.
- Functions: parameter types, a return type, a [row](abilities.md), and optional type parameters; parameter names are not part of the type.
- Unions and intersections.
- `never`, a subtype of every type, and `unknown`, a supertype of every type.
- Brands.
- `Cell<T>`.
- `Iterator<T>`, the type of an [iterator](iteration.md).
- Type parameters, each with an optional upper bound.

## Soundness

- An expression of static type `T` evaluates only to values `T` describes.
- No construct weakens that for convenience; where a sound rule and an ergonomic one conflict, the sound one wins.
- An operation that can miss, such as a lookup or an index, includes the miss in its type.
- A record merge takes from its second operand only the keys that operand's type lists.
- A key a record holds beyond its type therefore never replaces one of the first operand's with a value of unknown type.

## Subtyping

- Subtyping is structural, and coinductive over recursive types.
- A record is a subtype of any record with fewer keys or wider key types.
- Collections and iterators are covariant; immutability makes that sound.
- `Cell<T>` is invariant.
- A function type is a subtype of one with narrower parameters, a wider return type, or a larger row.
- Two function types with type parameters are related only if they take as many, with the same bounds; they are then related as they would be with one type standing for each parameter in both.
- Bounds must be the same, not one within the other, which keeps subtyping decidable.
- A function type with type parameters and one without are unrelated: subtyping never chooses a type argument.
- A literal type is a subtype of its primitive.
- A tuple and a list are unrelated, whatever they hold: a tuple is so many values, each with its own type, and a list is any number of one type.
- An intersection of records has the keys of both; an intersection with a brand adds the brand.
- An intersection of two different shapes is `never`; there are no overloaded function types and no negation types.
- `int` and `float` are unrelated.
- No type of one [shape](values.md) is a subtype of a type of another: a value has one shape.

## Singleton types

- A singleton type has exactly one value: a literal type, or a record or tuple whose members are all singletons.
- Anything that can be authored as a literal has a singleton type.
- A literal expression has its singleton type, and nothing but subtyping widens it; values are immutable, so there is no assignment to widen for.
- A [comptime](comptime.md) argument is known because its type is a singleton.

## Maps

- A map's entry type is a union of pairs, each a key type with a value type.
- A lookup by a key of type `K` yields the union of the value types of every pair whose key type overlaps `K`, or `null`.
- An inserted entry is a subtype of the entry type.
- Two types overlap unless they are provably disjoint.
- The relation between a key's type and its value's type is therefore never flattened into two independent unions.

## Unknown and casts

- `unknown` supports no operations.
- A cast tests a value against a type and yields either the value at that type or its alternative branch; it is the only way down from `unknown`.
- A cast decides by the value's [shape](values.md), so an erased type can be a cast target.
- A cast to a function type passes a function of exactly that type, not one whose type is within it: a program never decides subtyping as it runs; only checking does.
- A function type that mentions a type parameter is not a cast target, but inside a [cell](cells.md) type.
- A cast to a generic function type passes a generic function of exactly that type that holds no type arguments; one that needs none as it runs is the same value at every type, and passes however it was used.
- A cast to a type parameter is allowed, and tests against the type argument, which a generic function [receives at run time](backends.md).
- Only a function that casts to a type parameter, makes a cell at one, or hands one on to a function that does, needs its type arguments.
- A cast never adds a brand to a value.
- A cast to a handle type is decidable, because a handle carries the tag it was minted under.
- Values that enter from outside the program, [received values](exchange.md) included, enter as `unknown`.

## Narrowing

- Match is the only narrowing construct: its arms test literals, shapes, and types.
- A pattern is a wildcard, a literal, a record of patterns, a tuple of patterns, a type test, or a binder around another pattern.
- A record pattern matches any record that has at least its fields; a tuple pattern matches a tuple of exactly its length.
- A field a record pattern names that the subject's type does not mention is tested as `unknown`.
- A binder has the type of the values its pattern matches, not the type of the subject.
- Each arm sees the subject's type less what earlier arms always match.
- A match is exhaustive over the static type of its subject: after the last arm nothing is left.
- An arm that can match nothing is an error.
- What is left after an arm may be over-approximated, never under-approximated; a match the checker cannot prove exhaustive needs a final wildcard.
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
- An inferred type argument is the least type the arguments require: the union of what they supply for that parameter.
- An inferred argument that violates its bound is an error; inference never widens to the bound.
- Inside its definition a type parameter is known only by its bound, and an unbounded one supports no operations.
- A function type may take type parameters of its own, each with an optional bound; their names are not part of the type.
- A generic function named where nothing is expected of it is a value of such a type, and is still generic wherever it is passed or kept.
- A value of a generic function type is instantiated by the arguments it is called with, by the function type expected of it where that is not generic, or by type arguments written out.
- Instantiating is something done to a value where it is used, never a relation between types.
- TODO: a lambda and a [method](methods.md) cannot take type parameters yet, so only a definition is a generic function.
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
