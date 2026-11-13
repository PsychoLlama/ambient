# Core tree

The core tree is the checked form of a definition and the only form that is hashed, stored, exchanged, and compiled. This page fixes what it contains.

## Shape

- A core definition is a tree and a slot table.
- The tree refers to other definitions only through slots, numbered in order of first use.
- The slot table says what each slot refers to: a [symbol](symbols.md) while a workspace is being built, a hash once the definition is hashed.
- A local is a number assigned in binding order within its function; a core tree holds no names.
- A core tree holds no [metadata](trees.md); metadata is kept beside it, addressed by node position.
- Every type a later stage needs is explicit in the tree; nothing is inferred after checking.

## Open and closed

- A core definition whose slots are symbols is open; one whose slots are hashes is closed.
- Checking yields an open definition, and never learns a hash.
- The [hash](definitions.md) of a definition covers its tree with each slot replaced by the hash it refers to.
- A union's members are hashed in the order of those replaced forms; the order an open tree stores them in, and so its slot numbering, is not content.
- Compiling reads the tree and the [interfaces](definitions.md) of its slots, never their hashes; compiled code reaches a slot through its closed table.
- An edit to a dependency's body therefore changes a dependent's closed table and nothing else about it.
- Only closed definitions leave a workspace.

## Types

- Primitive types, `never`, and `unknown` are leaves.
- A literal type holds the literal.
- Record, tuple, list, set, map, function, union, intersection, brand, and cell types are nodes over their component types.
- A record type's fields are ordered by key, so field order is not content; fields are ordered by name and handlers by [tag](definitions.md).
- A function type holds its type parameters with their bounds, its parameter types, its return type, and its row.
- A row is a set of slots, each an ability; row aliases are expanded.
- A row's members are ordered by [tag](definitions.md), so the order they were written in is not content and neither are their names.
- A reference to a type definition is a slot and its type arguments.

## Expressions

- Literals: null, booleans, integers, floats, strings, bytes, and tags.
- A local, and a reference to a definition through a slot with its type arguments.
- A reference to a generic function with no type arguments is a value of a generic function type; an instantiation holds such a value and its type arguments.
- A closure: a slot for the lambda's definition, the locals it captures, and the [bindings it captures](abilities.md).
- Inside a lambda a captured value is addressed by its position among the captures, separately from the lambda's own locals.
- A lambda has the type parameters of the definition it is written in, and a closure passes them through unchanged.
- A call holds its callee and arguments.
- A perform holds an ability slot, arguments, and whether the ability [returns](handlers.md).
- A provide holds its mode, its body, and each handler with its ability slot.
- Binding a function value [in place](abilities.md) has no node: it appears as a closure that captures the value and the bindings.
- Constructions of records, tuples, lists, sets, and maps; a record construction keeps the order its fields were written in, because that is their evaluation order.
- Field access, match, cast, and a block of bindings ending in an expression.
- A record merge holds both records and the keys taken from the second, which are those its type lists.
- Provide, boundary, budget, cleanup, and the task constructs each have one node.
- A [recording](handlers.md) has no node: it appears as a provide of the handlers derived for it.
- A primitive operation holds the operation and its arguments; primitives have no slots.
- A step holds the [iterator](iteration.md) it steps; it is not a primitive, since it may call.
- A [comptime](comptime.md) call does not appear; its result does.

## Limits

- A tree nests no deeper than a fixed limit, counted in expressions, types, and patterns.
- A deeper tree is not a tree: a builder refuses to make it, checking refuses to produce it, and decoding refuses to read it.
- A binding nests what follows it, so the limit also bounds how many bindings a function makes in a row.

## Encoding

- A core tree has exactly one encoding, and equal trees encode to equal bytes.
- A hash is computed over the encoding, never over an in-memory form.
- The encoding names its format version; a change to the node set or the encoding is a new version, and changes every hash.
