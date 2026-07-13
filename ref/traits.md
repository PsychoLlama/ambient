# Traits and Impls

Part of the [Ambient Language Reference](architecture.md).

Traits define shared behavior for types. Only nominal types can implement
traits. (Types also take methods directly, without a trait — see
[Inherent Impls](#inherent-impls).) Traits also constrain generics — see
[Generic Constraints](#generic-constraints).

## Defining Traits

Traits are **nominal**, exactly like structs, enums, and abilities: the
mandatory `unique(<uuid>)` prefix _is_ the trait's identity. Everything
semantic — impl coherence, dispatch symbols, trait bounds, operator
desugaring — keys off the uuid, never the name, so renaming or moving a
trait never changes what an impl or a bound means, and two same-shaped
traits in different packages never unify.

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D01) trait Show {
  fn show(self): String;
}

unique(D098767B-4093-4D5C-BA37-AD92AA7B5D02) trait Describe {
  fn describe(self, prefix: String): String;
}
```

The `Self` type refers to the implementing type. The operator traits
(`Add`, `Eq`, ...) are already declared in `core::traits` with reserved
identities — implement those, don't redeclare them (see
[Prelude Traits](#prelude-traits)).

A trait method may declare its own type parameters, and those parameters may
carry trait bounds (`fn same<U: Eq>(self, a: U, b: U): Bool`). The bound
threads a hidden dictionary through the call, exactly like a bounded free
function — see [Generic Constraints](#generic-constraints). The current
limit is that a bounded trait method dispatches only on a **concrete
receiver**: calling it through a generic type-parameter receiver, or using a
conditional impl of such a trait as a dictionary source, is rejected (the
method's own dictionaries cannot yet thread through a fixed-arity dictionary
slot).

Two trait-declaration features parse but are **not supported yet** and are
rejected at the declaration site: trait-level type parameters (generic
traits, `trait Container<T>`) and supertraits (`trait Sub with Base`). A
method-level type parameter (`fn method<T: Bound>(...)`) covers many cases a
generic trait otherwise would.

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
// (`Default` is already declared in core::traits; shown here for shape.)
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
`<type-uuid>::<default-trait-uuid>::default` symbol as any impl method, with no receiver
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
  silent ambiguity; trait dispatch ambiguity between two _traits_ is still
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

The operator traits (`Add`, `Sub`, `Mul`, `Div`, `Mod`, `Eq`, `Ord`) plus
`Show` are part of the prelude: they are always in scope, and implementing an
operator trait enables the corresponding operator. They are ordinary
declarations in `core::traits`,
re-exported onto the prelude (`pub use core::traits::{Add, …, Ord};` in
`core_lib/prelude.ab`) like every other global — there is no separate
hardcoded copy. What _is_ special is their identity: each claims a reserved
uuid (`TRAIT_ADD_UUID` and friends, the `FFFF…-0010` block), and operator
desugaring anchors on those uuids. A module that declares its own trait
with the same name shadows the prelude entry for `use` and `impl` purposes,
but it can never capture an operator — `+` always means the reserved `Add`.
A declaration claiming a reserved trait uuid must match the canonical
name and shape exactly (`validate_reserved_trait`), the same hijack guard
reserved enums and primitives get.

The primitives implement the operator traits in `core::traits`
(`impl Eq for Number`, `impl Ord for Number`, `impl Ord for String`,
`impl Add for String`, ...). `Ord for String` compares lexicographically
through the `core::primitives::string::compare` native, so `List::sort`
and `min_of` work on strings as well as numbers.
These impls exist to satisfy trait bounds — `min_of(7, 3)` works because
Number has an Ord dictionary — while concrete operator uses on primitives
(`1 + 2`) always compile to the builtin opcodes, never through the impls
(whose bodies _are_ those builtins).

`Show { fn show(self): String }` is a general-purpose stringifier — the
minimum a value needs to be rendered. It has no operator either, but unlike
`Default` it _is_ prelude-exported, because it is the bound on
`Exception::throw<E: Show>` (see [abilities.md](abilities.md#error-handling)):
every thrown value must be renderable, so an uncaught or logging handler
always has a string. `core::traits` provides `impl Show` for `String`
(renders as itself), `Number`, `Bool`, and `Binary` (via the host
`to_string`); implement it on your own types to throw them. It claims the
reserved uuid after the operator block (`TRAIT_SHOW_UUID`, `FFFF…-0018`).

`Default` lives in `core::traits` too but is _not_ in the prelude: it has no
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

## Generic Constraints

Type parameters take trait bounds, Rust-style, on functions, impl blocks,
impl methods, trait methods, and ability methods — the same syntax in every
position:

```ambient
fn min_of<T: Ord>(a: T, b: T): T {
  if a < b { a } else { b }        // `<` dispatches through T's Ord bound
}

fn same<T: Eq>(a: T, b: T): Bool {
  a.eq(b)                          // bound methods are callable directly
}

impl<T: Eq> List<T> {
  fn contains(self, item: T): Bool { ... }
}

unique(...) ability Chooser {
  fn pick_equal<T: Eq>(a: T, b: T): Bool { a.eq(b) }
}
```

Multiple bounds join with `+` (`<T: Eq + Ord>`). Bounds
belong where generic _code_ lives: type declarations (`struct`, `enum`,
`type`) and `extern fn`s reject them.

Every position that takes inline bounds also accepts a trailing `where`
clause — functions, impl blocks, and trait / impl / ability methods:

```ambient
fn cmp_them<T>(a: T, b: T): Number where T: Ord { a.cmp(b) }
impl<T> Wrapper<T> where T: Eq { ... }
```

`where` is pure surface syntax: at lowering each clause folds into the named
type parameter's bounds, so `fn f<T>() where T: Eq` and `fn f<T: Eq>()` lower
to the identical AST. A clause may only constrain one of the declaration's own
type parameters — constraining a concrete type or an unknown name
(`where Number: Eq`) is a declaration-site error.

On a function or method the clause sits **after the return type and before the
`with` effect clause**, so the two never clash:

```ambient
fn f<T>(x: T): T where T: Eq + Ord with Stdio { x }
//              └ return ┘ └── where ──┘ └ with ┘
```

Inside a bounded body, the bound is what makes the parameter usable: a
bound's methods are callable on values of the parameter type, and the
operator sugar works when the bound is the corresponding reserved trait
(`T: Ord` enables `<`, `T: Eq` enables `==`). Calling a bounded generic
requires the argument type to satisfy every bound — either a concrete type
with a matching impl in the build, or a type parameter of the _caller_
that declares the same bound.

### Dictionaries, not monomorphization

A bounded function compiles **once** — the VM's uniform value
representation needs no per-type copies. Instead the function takes one
hidden trailing _dictionary_ parameter per bound: a tuple of function
values, the trait's methods in a canonical order. A bound-method call in
the body is a tuple access plus an indirect call. At a call site with a
concrete type, the checker resolves the impl and the compiler builds the
dictionary from the impl's method symbols — resolved through the ordinary
name→hash table, so the **call site's content hash pins the exact impl
methods it dispatches to**, exactly like direct calls. A generic caller
forwards its own dictionary parameter instead. Content addressing gains no
new channels: dictionary construction and slot access are ordinary
bytecode, covered by the existing hashes.

A **conditional (generic) impl** — `impl<T: Eq> Eq for Pair<T>`, or an
applied target like `impl Eq for Option<Number>` — is also a dictionary
source. Solving `Pair<Money>: Eq` unifies the impl's target against the
concrete type to recover its type-parameter assignments (`T = Money`),
recursively solves the impl's own bounds against them (`Money: Eq`), and
builds a dictionary whose slots are closures over those inner
dictionaries: each slot forwards its value arguments plus the captured
inner dictionaries to the impl method (which compiles with those exact
trailing dictionary parameters). Coherence stays at head-uuid granularity
— one impl per `(trait, head)` — so `impl<T> Eq for Pair<T>` and `impl Eq
for Pair<Number>` conflict; a wrong instantiation (`Option<String>` against
`impl Eq for Option<Number>`) simply fails to match and reports an
unsatisfied bound. Recursion is depth-limited against pathological nesting.

A bounded generic can also be used as a **first-class value** — bound to a
`let`, or passed to a higher-order function — as long as the instantiation is
concrete at the reference. Wherever the checker can fix the type argument
(from an annotation, or from how the value is used), it solves the bound and
the compiler lowers the bare reference to a closure that captures the
resolved dictionaries and forwards its value arguments plus those
dictionaries to the function — the same closure shape a conditional impl's
dictionary slots use, and hash-linked like a direct call. Every dictionary
source works (a concrete impl, a forwarded enclosing dictionary — inside
lambdas too — or a conditional impl). The one residual limit is a genuinely
ambiguous binding whose type nothing fixes (`let f = same;` with no further
information): the constrained variable generalizes away from its constraint,
and the checker reports it as a clear compile error with an "add an
annotation" hint — never a miscompile.

## Dispatch, Coherence, and Content-Addressing

Method calls dispatch statically: the receiver's concrete type is known
during type checking, which resolves the call to a canonical method symbol
— `<type-uuid>::<trait-uuid>::<method>` for trait methods (both identities,
so two same-named traits implemented for one type can never collide), or
the two-segment `<type-identity>::<method>` for inherent methods, where the
identity is the UUID for any nominal type — a `unique struct`, a declared
enum, or the reserved-name prelude enums `Option`/`Result` (which carry
fixed UUIDs) — and the head name for the built-in containers, which have no
UUID (`List::fold`, `Map::get`). The segment counts differ, so the two
families can never collide. Impl methods
compile as ordinary named functions under their symbol, so they are
content-addressed exactly like any other function (hash = bytecode +
constants + dependency hashes), and call sites link against the content
hash. There is no runtime trait registry and no dynamic dispatch — bounded
generics included: their dictionaries are built at (hash-pinned) call
sites, not looked up at runtime.

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
_invariant_ depends on impl behavior (say, a set ordered by `Ord::cmp`)
can be built by one version and queried by another that compares
differently. That hazard predates inherent impls — any trait impl edit
plus surviving state can trigger it — and it is a state-handoff problem,
owned by the live-upgrade model ([live-upgrade.md](live-upgrade.md),
"Migration"), not by the impl system. Coherence within one build plus
hash-pinned dispatch across builds is the whole mechanical story.
