# Types and Nominal Identity

Part of the [Ambient Language Reference](architecture.md).

## Types

```ambient
// Primitives
Number    // 64-bit float (f64)
String    // UTF-8 string
Bool      // true, false
Binary    // immutable byte buffer

// Composite
{ x: Number, y: Number }           // Records (structural)
(Number, String, Bool)             // Tuples
List<T>, Set<T>, Map<K, V>         // Collections

// Enums (tagged unions) are nominal: every declaration carries a
// mandatory `unique(<uuid>)` prefix, so two structurally identical enums
// are distinct types (see Nominal Enums below). Option and Result are
// ordinary declarations in core (with fixed reserved UUIDs) whose
// constructors (Some, None, Ok, Err) are always in scope via the prelude.
unique(E1B2C3D4-0000-0000-0000-000000000001) enum Shape { Circle(Number), Square(Number), Dot }

// Construct with the variant name; destructure with match. In pattern
// position, bare uppercase-initial identifiers are variant patterns
// (None, Dot); lowercase identifiers are bindings.
fn area(s: Shape): Number {
  match s {
    Circle(r) => 3 * r * r,
    Square(side) => side * side,
    Dot => 0,
  }
}

// Nominal types (structurally identical but incompatible)
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: String }

// Generics; type parameters take trait bounds (see traits.md)
fn identity<T>(x: T): T { x }
fn min_of<T: Ord>(a: T, b: T): T { if a < b { a } else { b } }
```

## Nominal Types

A `unique(<uuid>) struct` declaration gives a type its own identity, distinct
from any structurally identical type. That identity _is_ the UUID:

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: String }
```

UUID literals are written in canonical `8-4-4-4-12` form and **must be
uppercase** (`0-9`, `A-F`). Uppercase is a lexical requirement, not a
convention: the lexer recognizes an uppercase UUID as a single token, which
keeps it unambiguous against identifiers, numbers, and future lowercase `0x`
hex literals. A lowercase or malformed UUID is a syntax error. The stored
value is canonicalized to lowercase for content addressing and display; only
the _source syntax_ is uppercase.

## Extern Types

An `extern unique(<uuid>) struct` is a nominal type whose _values_ are
provided by the host: user code may name it, annotate with it, and read
it, but never construct one (`String {}` is rejected). The four
primitives are the canonical case — `pub extern unique(…FF03) struct
String;` in `core_lib/primitives/string.ab` — ordinary in-language
declarations whose reserved UUIDs are anchored against the engine
(`validate_reserved_struct`), so the sources and the engine can never
drift.

Extern _types_ keep their UUID in source (type identity is a
checking-time concept — an editor must resolve it without host bindings),
while extern _functions_ get their UUIDs from the host binding at compile
time (implementation identity is a link-time concept) — see
[core-library.md](core-library.md#native-functions-extern-fn).

## Nominal Enums

Enums are nominal too, and their `unique(<uuid>)` prefix is **mandatory** —
a bare `enum` is a compile error. This is deliberate: an enum's identity is
its UUID, not its shape or its name, so two structurally identical enums (or
two enums with the same name in different packages) are distinct,
non-interchangeable types.

```ambient
unique(E1B2C3D4-0000-0000-0000-000000000001) enum Shape {
  Circle(Number), Square(Number), Dot
}

unique(E1B2C3D4-0000-0000-0000-000000000002) enum Tree<T> {
  Leaf, Node(T)
}
```

An enum is otherwise an ordinary named type constructor: it can be generic,
its variant constructors and patterns are in scope in the declaring module,
and it carries inherent methods (`impl Shape { ... }`,
`impl<T> Tree<T> { ... }`). Because the identity is the UUID, an enum's
inherent methods get uuid-based dispatch symbols (`<uuid>::method`) exactly
like a nominal `struct`'s — see [Dispatch, Coherence, and
Content-Addressing](traits.md#dispatch-coherence-and-content-addressing) — so a
same-named enum elsewhere can never claim them.

`Option<T>` and `Result<T, E>` are nominal on the same footing: they carry
fixed reserved UUIDs (`OPTION_UUID`/`RESULT_UUID`), so they are as distinct
and non-interchangeable as any declared enum. Their canonical declarations
are ordinary Ambient source — `pub unique(…FFFF0001) enum Option<T>` in
`core_lib/option.ab`, likewise `Result` in `core_lib/result.ab` — alongside
their combinators and predicates, exposed as inherent methods (`map`,
`and_then`, `is_some`, `unwrap_or`, `is_ok`, …). What makes them special is
the _prelude_: `core_lib/prelude.ab` re-exports the two enums and their
variants (`pub use core::option::{Option, Some, None};`, likewise `Result`),
and `ModuleRegistry::inject_prelude` folds every such re-export into every
module's scope at lowest precedence — the resolver-level equivalent of a
`use prelude::*` at the top of each module. That is why `Some`, `None`, `Ok`,
and `Err` need no import anywhere, and it is the _same_ mechanism that carries
the four primitives and the operator traits: a global value is just a
shorthand for its fully-qualified `core::…` path, re-exported onto the
prelude. Reserved UUIDs cannot drift from the source: a declaration that
claims one must match the reserved name, type parameters, and variant layout
exactly (`validate_reserved_declaration`, which reads a small validation-only
spec — not a second registry seed), so the core sources fail the build if
they diverge and no other module can hijack a reserved identity for a
different shape.
