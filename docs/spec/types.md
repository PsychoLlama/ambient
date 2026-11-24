# Types

Types are structural, erased at runtime, and content-addressed. A type is checked and cached without reference to any function.

## Forms

- Primitives: `null`, `bool`, `int` (64-bit signed), `float` (64-bit), `string` (UTF-8), `bytes`.
- Literal types: a single `bool`, `int`, `float`, `string`, or `bytes` value, or a single [tag](definitions.md).
- `Tag`, the type of every tag value.
- Records: a set of keys, each with a type; a key is a field name, a [contract](methods.md), or an [ability](handlers.md).
- A record's key may hold a [method](methods.md), whose type also says what it needs of its receiver.
- Tuples: a fixed sequence of two or more types. Nothing to group is null, and one value grouped is that value.
- Collections: `List<T>`. A [map and a set](maps.md) are types of the standard library.
- Functions: parameter types, a return type, a [row](abilities.md), and optional type parameters; parameter names are not part of the type.
- Unions and intersections.
- `never`, a subtype of every type, and `unknown`, a supertype of every type.
- Brands.
- `Cell<T>`.
- `Iterator<T>`, the type of an [iterator](iteration.md).
- Type parameters, each with an optional upper bound.
- [Type matches](#type-matches).

## Soundness

- An expression of static type `T` evaluates only to values `T` describes.
- No construct weakens that for convenience; where a sound rule and an ergonomic one conflict, the sound one wins.
- An operation that can miss, such as a lookup or an index, includes the miss in its type.
- A record merge takes from its second operand only the keys that operand's type lists.
- A key a record holds beyond its type therefore never replaces one of the first operand's with a value of unknown type.

## Subtyping

- Subtyping is structural, and coinductive over recursive types.
- Two uses of one recursive definition are related if their arguments are, each the way the definition varies with it: with it, against it, both ways, or not at all.
- How a definition varies with a parameter follows from what it is written as; nothing states it.
- A definition that applies itself to a type built from its own parameter has no end of different unfoldings: two uses of it are related by their arguments and no other way, and it is unfolded only a few times against any other type.
- A method that needs the record type it is in makes that type invariant in whatever the record holds of a parameter, since the type is then on both sides of the method.
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

## Unknown and casts

- `unknown` supports no operations.
- A cast tests a value against a type and yields either the value at that type or its alternative branch; it is the only way down from `unknown`.
- What a cast gives is the union of its type and its alternative's. Where the two overlap, nothing says which it was, so the alternative is chosen outside the type by whoever needs to tell: a [tag](definitions.md) is always one.
- A cast decides by the value's [shape](values.md), so an erased type can be a cast target.
- A cast to a function type passes a function of exactly that type, not one whose type is within it: a program never decides subtyping as it runs; only checking does.
- A function type that mentions a type parameter is not a cast target, but inside a [cell](cells.md) type.
- A cast to a generic function type passes a generic function of exactly that type that holds no type arguments; one that needs none as it runs is the same value at every type, and passes however it was used.
- A function written inside a generic function may be of a type that depends on type arguments. A cast passes it if that type, at the arguments the function [holds](values.md), is the cast's.
- TODO: a generic function written inside a generic function passes no cast until it is instantiated: its type is generic and depends on type arguments at once.
- A cast to a type parameter is allowed, and tests against the type argument, which a generic function [receives at run time](backends.md).
- Only a function that casts to a type parameter, makes a cell at one, or hands one on to a function that does, needs its type arguments.
- A cast never adds a brand to a value.
- A cast to a handle type is decidable, because a handle carries the brand it was minted under.
- Values that enter from outside the program, [received values](exchange.md) included, enter as `unknown`.

## Narrowing

- Match is the only narrowing construct: its arms test literals, shapes, and types.
- A pattern is a wildcard, a literal, a record of patterns, a tuple of patterns, a type test, or a binder around another pattern.
- A record pattern matches any record that has at least its fields; a tuple pattern matches a tuple of exactly its length.
- A field a record pattern names that the subject's type does not mention is tested as `unknown`.
- A binder has the type of the values its pattern matches, not the type of the subject.
- A type no value can be tested for, as one that holds a [method](methods.md) is, is tested as the nearest type that can be: itself without its methods, and `unknown` for each part no test can tell.
- That stands only where it tells as much: what the nearer test lets through of the subject's type is all of the type that was written. Otherwise the test is an error.
- A [cast](#unknown-and-casts) to such a type is tested the same way, and gives what the test lets through.
- A test asks no more than tells its values from the rest of the subject's type: where a shape alone does, the shape is all that is tested, and a function is then tested only for being one.
- A function is tested for its type only where that leaves more to tell, and such a test never takes all of a type: a function says the one type it was written as, which may be within the type asked for.
- An arm is refused where it would take all of a member of the subject's type, but could tell that member's values only by asking a function in a record for its type: the arm could turn them away with nothing said. A record of other keys may hold the same ones, so `{ run: fn(): int }` is not told from `{ done: int }` by its shape.
  - TODO: allow such an arm. Nothing but the time to design it stands in the way, and it is wanted: a value that says its record's keys, or a function that says each type it is within, would decide it.
- Each arm sees the subject's type less what earlier arms always match.
- A match is exhaustive over the static type of its subject: after the last arm nothing is left.
- An arm that can match nothing is an error.
- What is left after an arm may be over-approximated, never under-approximated; a match the checker cannot prove exhaustive needs a final wildcard.
- Exhaustiveness stays sound under [exchange](exchange.md): a union with another member is a different type with a different hash, and foreign values arrive as `unknown`.

## Brands

- A brand is a type with an [identity](definitions.md) and no structure.
- `T & Brand` is a subtype of `T`; going the other way is the branding operation.
- Branding is permitted only in the module that declares the brand.
- Branding is written as a call of the brand with the value, and gives the value at its type and the brand; a literal's type is widened first, so `Duration(5)` is an `int & Duration`.
- A branded value is used as its type everywhere: it has that type's methods and operations, and what they give is not branded.
- A branded type cannot be tested for, since nothing of the brand is there as a program runs: a match on one tests the type under the brand, and only where that is enough to tell.
- Brands are erased: they constrain code compiled from source and are not a security boundary; see [exchange](exchange.md).
- A brand on its own is the type of a [handle](values.md).

## Intersections

- `A & B` of two record types is the record type with the fields of both, so it is a subtype of each.
- A field of both has the type that is both of its types: the same type, the intersection of two records, or whichever is a subtype of the other.
- An intersection is its record: nothing of it remains as a type of its own, so it is tested, shown, and hashed as that record.
- Only records and [brands](#brands) are intersected. Two types with no value in common make no type, and neither does an intersection with a type parameter, whose record is not known where it is written.

## Generics

- Type and function definitions take type parameters; a bound is a subtype bound.
- Type arguments are inferred at the use site or written explicitly.
- An inferred type argument is the least type the arguments require: the union of what they supply for that parameter.
- Where the result holds the parameter to more than being read, a literal type inferred for it is its primitive: a list made of `1` and `2` is a list of those, and a map made of them takes any integer.
- A parameter inside a union is inferred from the member an argument fits, so a node is accepted where a node or null is expected.
- An inferred argument that violates its bound is an error; inference never widens to the bound.
- A type definition applied to an argument outside its parameter's bound is an error wherever it is written, a bound mentioning the definition's other arguments as they are. A parameter of a generic function type is known there by its bound, and a binder of a [type match](#type-matches)'s arm by what its pattern narrows the subject to.
- Inside its definition a type parameter is known only by its bound, and an unbounded one supports no operations.
- A function type may take type parameters of its own, each with an optional bound; their names are not part of the type.
- A generic function named where nothing is expected of it is a value of such a type, and is still generic wherever it is passed or kept.
- A value of a generic function type is instantiated by the arguments it is called with, by the function type expected of it where that is not generic, or by type arguments written out.
- Instantiating is something done to a value where it is used, never a relation between types.
- A lambda may take type parameters, and is then a value of a generic function type; a parameter it gives no type takes one from a generic function type expected of it that has as many type parameters.
- A lambda has only the type parameters it writes: nothing expected of it makes it generic.
- A [method](methods.md) written as such a lambda is generic the same way.
- A recursive generic type may apply itself to types other than its own parameters: a method of a container may give the container at another type.

## Type matches

A type match computes a type from another as a match computes a value: by the same patterns, arm by arm. It is how a type depends on a type parameter; computation over closed types is [comptime](comptime.md)'s.

- A type match holds a subject type and arms, each a pattern and a type.
- Its patterns are those of a [match](#narrowing). A binder binds the type of what its pattern matched, for the arm's type to name.
- A test may name any type, functions and brands among them: nothing is tested as a program runs.
- Each member of the subject's union is taken by every arm that may match it, up to the first that always does, as a value would be. The type match is the union of what those arms give.
- Two types may match unless they are [disjoint](#overlap).
- A member no arm may match gives nothing, so a type match need not be exhaustive, and one over `never` is `never`.
- A type match whose subject or patterns name a type parameter is decided where the parameter is known, and is left as it is until then.
- A match on a value of type `T` gives the type match over `T` with the same patterns, each arm giving the type of the arm's value.
- So a generic function narrows a value of a type parameter without losing what it is related to: a binder's type is the type match that picks it out.
- A type match left as it is is within a type that every arm that may match is within, with each parameter at its bound.
- Nothing is within one but `never` and the same type match, related to it by their subjects and the types its patterns name, each the way the match varies with it.
- A value of one is therefore given only by a match with the same arms, or by what already has that type.
- A type match does not refer to itself, directly or through a [group](definitions.md).

## Overlap

- Two types are disjoint if no value is described by both, and overlap otherwise.
- Overlap is decided by the checker and errs toward overlapping: two types not proven disjoint overlap.
- Two brands on one type overlap, since brands are [erased](#brands) and a value may carry both.
- Two record types overlap unless a key both list has disjoint types, since a record may hold keys its type leaves out.

## Type definitions

- A non-recursive type definition is transparent: it has the hash of its expansion, and naming it changes nothing.
- A recursive type definition is a member of a [group](definitions.md).
- Equal hashes imply equal types; unequal hashes imply nothing, since a recursive type has many unfoldings.
- Subtyping decides type relations; hashes only shortcut and cache it.

## Checking

- Checking is bidirectional: headers supply the types of definitions, and everything else is inferred within one definition.
- Local bindings, lambda parameters, and type arguments are inferred; nothing in a header is.
- A subtyping judgement between two closed types is a pure function of their hashes.
