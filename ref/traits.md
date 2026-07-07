# Traits and Impls

Part of the [Ambient Language Reference](architecture.md).

Traits define shared behavior for types. Only nominal types can implement
traits. (Types also take methods directly, without a trait — see
[Inherent Impls](#inherent-impls).)

## Defining Traits

```ambient
trait Show {
  fn show(self): String;
}

trait Add {
  fn add(self, other: Self): Self;
}

trait Eq {
  fn eq(self, other: Self): Bool;
}
```

The `Self` type refers to the implementing type.

## Implementing Traits

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Money { cents: Number }

impl Show for Money {
  fn show(self): String {
    "$" + to_string(self.cents / 100)
  }
}

impl Add for Money {
  fn add(self, other: Money): Money {
    Money { cents: self.cents + other.cents }
  }
}

impl Eq for Money {
  fn eq(self, other: Money): Bool {
    self.cents == other.cents
  }
}
```

## Method Calls

Methods are called using dot notation:

```ambient
let m = Money { cents: 1500 };
let s = m.show();           // "$15"

let a = Money { cents: 100 };
let b = Money { cents: 50 };
let c = a.add(b);           // Money { cents: 150 }
```

## Operator Overloading

Standard operators dispatch to trait methods for nominal types:

| Operator | Trait | Method                         |
| -------- | ----- | ------------------------------ |
| `+`      | `Add` | `add(self, other: Self): Self` |
| `-`      | `Sub` | `sub(self, other: Self): Self` |
| `*`      | `Mul` | `mul(self, other: Self): Self` |
| `/`      | `Div` | `div(self, other: Self): Self` |
| `%`      | `Mod` | `rem(self, other: Self): Self` |
| `==`     | `Eq`  | `eq(self, other: Self): Bool`  |
| `!=`     | `Eq`  | `eq` (negated)                 |

```ambient
let a = Money { cents: 100 };
let b = Money { cents: 50 };
let c = a + b;              // Calls a.add(b)
let equal = a == b;         // Calls a.eq(b)
```

For primitive types (`Number`, `Bool`, `String`), operators use built-in implementations.

Trait method signatures carry no `with` clause, so **trait impl method
bodies must be pure** — the checker enforces it. An effectful method
would otherwise launder abilities past callers invisibly: dot dispatch
and operator overloading consult only the trait signature. (Effectful
shared behavior belongs in ordinary functions, or awaits effect-carrying
trait signatures as a language extension.)

## Associated Functions

A trait method whose first parameter is not `self` is an _associated
function_: it belongs to the type but takes no receiver. It is called with
the `Type::method(...)` path form rather than dot notation, because there is
no value to dispatch on — the leading path segment names the implementing
type, which the checker resolves to the impl's method symbol:

```ambient
trait Default {
  fn default(): Self;
}

impl Default for Money {
  fn default(): Money { Money { cents: 0 } }
}

let m = Money::default();     // Money { cents: 0 }
let c = Money::default() + Money { cents: 5 };  // associated calls are ordinary expressions
```

Dispatch is still static: `Money::default()` resolves to the same canonical
`<type-uuid>::Default::default` symbol as any impl method, with no receiver
pushed at the call site.

## Inherent Impls

An `impl` block without a trait attaches methods directly to a type. This
is how a type grows an API that isn't shared behavior — no trait ceremony
required:

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Money { cents: Number }

impl Money {
  fn double(self): Money {
    Money { cents: self.cents * 2 }
  }
  fn from_dollars(d: Number): Money {   // no self: associated function,
    Money { cents: d * 100 }            // called as Money::from_dollars(3)
  }
}
```

Inherent impls are not limited to `unique struct` declarations. Declared
enums (nominal, keyed by UUID), the reserved-name prelude enums (`Option`,
`Result`), the built-in containers (`List`, `Map`, `Set`), and the
primitives can all carry methods, and impls may be generic over the
target's type parameters:

```ambient
impl<T> Option<T> {
  fn get_or(self, fallback: T): T {
    match self { Some(v) => v, None => fallback }
  }
}

let x = Some(41).get_or(0);   // receiver's type arguments instantiate T
```

Rules:

- **Signatures are contracts.** Parameter and return types are declared in
  full (the return type is mandatory); `Self` refers to the target type.
- **Effects are declared.** An inherent method takes a `with` clause like a
  function; no clause means pure, enforced on the body and required at
  call sites. (Trait impl methods don't take `with` clauses — their
  signatures come from the trait.)
- **One definition per method per type.** Several impl blocks for one type
  merge, but a second definition of the same method name for the same type
  — anywhere in the build — is a coherence error. Core already claims
  `Option::map`, so a program cannot redefine it; new method names on core
  types are fine.
- **Inherent wins.** If a type has both an inherent method and a trait
  method of the same name, dot dispatch resolves the inherent one (as in
  Rust). Adding an inherent method is a deliberate local override, never
  silent ambiguity; trait dispatch ambiguity between two *traits* is still
  an error at the call site.
- **No blanket impls.** The target must be a concrete type identity —
  `impl<T> T` is rejected, as are structural targets (records, tuples,
  functions), which have no identity to attach methods to.

The core library uses inherent impls to expose its Option/Result/List
combinators as methods (see `crates/ambient-engine/src/core_lib/*.ab`) — the
canonical core API — so chains read left to right:

```ambient
[1, 2, 3].map((x) => x * 10).fold(0, (acc, x) => acc + x)   // 60
Some(20).map((v) => v * 2).unwrap_or(0)                     // 40
```

The qualified module-function call form (`Module::f(x, ...)`) remains a
general language feature for user code — a method call is just the
receiver-first spelling of the same content-addressed function. Core itself,
however, no longer publishes its combinators as free companion functions; the
only free functions it keeps are the two with no method form: `List::range`
(no receiver) and `Option::flatten` (its receiver would be
`Option<Option<U>>`, inexpressible in `impl<T> Option<T>`).

## Prelude Traits

The operator traits (`Add`, `Sub`, `Mul`, `Div`, `Mod`, `Eq`, `Ord`) are
part of the prelude: they are always in scope, and implementing one enables
the corresponding operator. They are ordinary declarations in `core::traits`,
re-exported onto the prelude (`pub use core::traits::{Add, …, Ord};` in
`core_lib/prelude.ab`) like every other global — there is no separate
hardcoded copy. A module that declares its own trait with the same name
shadows the prelude entry.

`Default` lives in `core::traits` too but is *not* in the prelude: it has no
operator that desugars to it, so it is standard-library convenience rather
than a load-bearing global. Using it requires an explicit
`use core::traits::Default;`. It supplies a canonical value for a type via the
associated function `default(): Self` (see
[Associated Functions](#associated-functions)):

```ambient
trait Default {
  fn default(): Self;
}
```

The `Ord` trait is used for comparison operators:

```ambient
trait Ord {
  fn cmp(self, other: Self): Number;  // -1, 0, or 1
}
```

Comparison operators adapt the trait method's result: `!=` negates
`Eq.eq`, and `<`, `<=`, `>`, `>=` compare `Ord.cmp`'s result against 0.

## Dispatch, Coherence, and Content-Addressing

Method calls dispatch statically: the receiver's concrete type is known
during type checking, which resolves the call to a canonical method symbol
— `<type-uuid>::<Trait>::<method>` for trait methods, or the two-segment
`<type-identity>::<method>` for inherent methods, where the identity is the
UUID for any nominal type — a `unique struct`, a declared enum, or the
reserved-name prelude enums `Option`/`Result` (which carry fixed UUIDs) —
and the head name for the built-in containers, which have no UUID
(`List::fold`, `Map::get`). The segment counts differ, so the two families
can never collide. Impl methods
compile as ordinary named functions under their symbol, so they are
content-addressed exactly like any other function (hash = bytecode +
constants + dependency hashes), and call sites link against the content
hash. There is no runtime trait registry and no dynamic dispatch.

Traits and impls declared anywhere in the build are visible to every
module in it. Coherence is enforced at exactly the granularity of the
dispatch symbol: one impl per `(trait, type)`, one inherent definition per
`(type, method name)`, across the build closure — the modules reachable
from the entry point. A local check ("this registration found no
duplicate") is sufficient to guarantee the global invariant ("every call
site resolves this symbol to one implementation"), because the symbol
embeds the type's identity and resolution consults nothing outside the
build.

### Why this survives live upgrade

In Rust, coherence must be global forever because dispatch is resolved
through type-indexed tables that all code shares — two crates disagreeing
about `impl Hash for K` would silently corrupt any `HashMap<K, _>` they
exchange. Ambient's constraint set is different: a live upgrade can
replace `impl Money` in a running system without touching `Money`, so
"one impl per type, ever" is not even expressible. It is also not needed:

- **A call site is pinned to an implementation by hash, not by name.**
  Type checking resolves the symbol, compilation freezes the callee's
  content hash into the caller (the caller's own hash covers it). There is
  no later lookup that could see a different impl.
- **Upgrades replace call sites, not tables.** Shipping a new
  `impl Money` produces new method hashes and re-links the callers that
  should use them — themselves new hashes. Old code keeps calling the old
  methods; both versions can coexist in one store, one VM, one wire
  transfer. This is exactly how plain functions already behave under
  content addressing; impls add nothing new to break.
- **Values don't carry their impls.** Nominal types are erased at runtime
  (a `Money` is just its record), so data constructed by old code is
  fully usable by new methods and vice versa, as long as the type's shape
  agrees — which is the type's identity question (its UUID), not the
  impl's.

The residual hazard is semantic, not mechanical: a data structure whose
*invariant* depends on impl behavior (say, a set ordered by `Ord::cmp`)
can be built by one version and queried by another that compares
differently. That hazard predates inherent impls — any trait impl edit
plus surviving state can trigger it — and it is a state-handoff problem,
owned by the (experimental) process model ([processes.md](processes.md)),
not by the impl system. Coherence within one build plus hash-pinned
dispatch across builds is the whole mechanical story.
