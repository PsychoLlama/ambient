# Ambient Language Reference

A content-addressed, ability-based programming language inspired by Unison, Rust, and TypeScript.

## Design Philosophy

- **Content-addressed**: Functions identified by hash of implementation + type signature
- **Pure with algebraic abilities**: Side effects explicit via delimited continuations
- **Remote-capable**: Functions serializable for remote execution or live program replacement
- **Immutable**: All values are immutable; state modeled through ability handlers

## Syntax

### Basic Structure

Expression-based with C-like syntax. No statements, only expressions. Semicolons required.

```ambient
// Constants map an identifier to a single hashed primitive value. The
// initializer must be a literal (number, string, boolean, `()`, or a negated
// numeric literal) — not an identifier, call, or compound expression. The
// value is baked in when the module is built and inlined at each reference.
// This is deliberately minimal and may widen later.
const PI: number = 3.14159;

// Functions
fn add(x: number, y: number): number {
  x + y
}

// Public functions (exported)
pub fn multiply(x: number, y: number): number {
  x * y
}
```

### Modules

Modules map 1:1 to files under `src/`; a directory is a namespace module
whose members are its children, so `src/net/http/client.ab` is the module
`pkg::net::http::client` with no `mod.rs`-style ceremony. Path roots:
`pkg` (package root), `self` (same directory), `super` (parent directory,
chainable), `core` (standard library), `platform` (host bindings).

**Every item in a build has exactly one fully-qualified identity** —
`<defining module>::<name>` — and the two access rules follow from it:

1. Anything reachable by its fully-qualified path works through `use`,
   and vice versa.
2. However a reference is spelled — bare imported name, module-alias
   path, inline rooted path — it resolves to that one identity. The
   compiler front end canonicalizes every reference before checking
   (`crates/ambient-engine/src/resolve.rs`), so the checker, the
   intrinsic tables, the ability resolver, and the linker all key off
   the same canonical name.

`use` takes a Rust-style use-tree:

```ambient
use pkg::utils::helper;                 // Item import: `helper` as a bare name
use pkg::utils::{a, deep::{b, c}};      // Brace groups, nested arbitrarily
use core::primitives::Number::sqrt as root2;        // `as` renames the local binding
use {core::primitives::Number, platform::Stdio};    // Root-level groups
use self::utils;                        // Whole-module import: utils::helper(...)
use pkg::net::http;                     // Directory namespaces import too:
use http::client::get;                  //   ...and a module alias can root
                                        //   another use (any order; resolved
                                        //   by fixed point)
use pkg::shapes::Shape;                 // Enum import: type, constructors, patterns
```

Braces are pure grouping — the tree flattens during lowering, so
`use a::b::{c, d};` is exactly `use a::b::c; use a::b::d;`. A flattened
path names an entity by its final segment, and the resolver binds every
namespace meaning that exists: a submodule, a value, a type, an ability.
Modules, values, types, and abilities occupy separate namespaces resolved
by syntactic position, so a name that is both a submodule and an exported
item binds both and the use site disambiguates (`c(...)` is the value,
`c::foo` the module).

`use` is also a statement: a block-scoped import binds from its statement
to the end of the enclosing block, types as nothing, and compiles to
nothing.

```ambient
pub fn hyp(a: number, b: number): number {
  use core::primitives::Number::sqrt;
  sqrt(a * a + b * b)
}
```

Inline fully-qualified paths need no import anywhere a name can appear:
expressions (`pkg::utils::helper(1)`), type positions
(`pkg::shapes::Money`, `core::collections::List<number>`), effect rows
(`with platform::Stdio`, `with pkg::effects::Counter`), performs, handler
arms, and sandbox clauses. Local bindings shadow module-level names,
which shadow imports, which shadow the prelude.

`pub` gates every export: functions, consts, types, enums, abilities, and
traits are module-private unless declared `pub`. A failed import — missing
module, missing symbol, or private symbol — is a compile error at the `use`
item, never a silent no-op. Importing an enum brings its variant
constructors and patterns into scope wholesale, as if declared locally;
a single variant cannot be imported on its own, and a qualified *type*
reference alone (`pkg::shapes::Shape` in a signature) does not bring
constructors into scope — import the enum where you construct or match
it. `pub use` re-exports items (and whole modules), and imports through a
re-export resolve (and link) to the module that defines the symbol.
Re-export paths must be rooted (`pkg`/`core`/`platform`/`self`/`super`),
not alias-relative, so downstream modules can resolve them without this
module's scope.

Core modules (`core::collections::List`, `core::primitives::Number`, `core::primitives::String`) are ordinary
Ambient modules — compiled, content-addressed, and stored exactly like
user code (see `crates/ambient-engine/src/core_lib/`). Beneath them sits
a fixed set of _intrinsics_ (`core::primitives::Number::sqrt`, `core::collections::List::length`,
`core::primitives::String::concat`, ...) that compile to dedicated opcodes; an
intrinsic is an ordinary item of its module — importable, aliasable,
reachable through `use core::primitives::Number;` + `Number::sqrt(x)` — and takes
precedence over a compiled function at the same path. `core` is a keyword
and `platform` a contextual keyword, and a user module may not take
either name (the build rejects `src/core.ab` / `src/platform.ab`), so the
reserved namespaces can never be shadowed.

The `core::` hierarchy is defined by the `core_lib/` source tree itself, not
by a hand-maintained list: `register_core_modules` walks the embedded tree and
maps every `.ab` file to a module path through the *same* file↔module mapping
(`ModulePath::from_relative_file_path`) that user packages use, and each
directory's `main.ab` (a *directory module*) groups its siblings with `pub use
self::…`. A directory module anchors `self`/`super` at its own path rather than
its parent, so `core_lib/collections/main.ab` re-exports its neighbours as
`core::collections::…`. Intrinsic opcodes are the one Rust-side coupling: their
table (`compiler/intrinsics.rs`) keys off the resolved module path, so it must
name the same paths the tree registers.

A core module that backs a type takes that type's PascalCase name: `List`,
`Option`, `Result`, and `Number` are the companion modules of the `List<T>`,
`Option<T>`, `Result<T, E>`, and `Number` types, so `List::range` reads as an
associated function of `List` and `Number::sqrt` as one of `Number`. Related
modules group under lowercase namespace parents — `core::collections` (`List`,
`map`, `set`), `core::primitives` (`String`, `Number`, `Bool`, `Binary`) — while
top-level namespaces like `time`, `convert`, and `reflect` name no single type.
Types, values, and modules occupy separate namespaces resolved by
syntactic position, so the type `List` and the module `core::collections::List` coexist
without ambiguity.

Known gaps (deliberate, minor): qualified references to *generic* type
aliases and generic `unique` types are unresolved (parameter substitution
is checker work); ability names inside function *type* annotations
(`(T) -> U with E`) accept only bare or `platform::` spellings; intrinsics
are not first-class values (`let f = core::primitives::Number::sqrt;` is an error —
call them directly).

A local binding shadows a module alias: after `let utils = ...;`,
`utils.method()` is a trait-method call on the value.

### Types

```ambient
// Primitives
number    // 64-bit float (f64)
string    // UTF-8 string
bool      // true, false

// Composite
{ x: number, y: number }           // Records (structural)
(number, string, bool)             // Tuples
List<T>, Set<T>, Map<K, V>         // Collections

// Enums (tagged unions) are nominal: every declaration carries a
// mandatory `unique(<uuid>)` prefix, so two structurally identical enums
// are distinct types (see Nominal Enums below). Option and Result are
// ordinary declarations in core (with fixed reserved UUIDs) whose
// constructors (Some, None, Ok, Err) are always in scope via the prelude.
unique(E1B2C3D4-0000-0000-0000-000000000001) enum Shape { Circle(number), Square(number), Dot }

// Construct with the variant name; destructure with match. In pattern
// position, bare uppercase-initial identifiers are variant patterns
// (None, Dot); lowercase identifiers are bindings.
fn area(s: Shape): number {
  match s {
    Circle(r) => 3 * r * r,
    Square(side) => side * side,
    Dot => 0,
  }
}

// Nominal types (structurally identical but incompatible)
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: string }

// Generics
fn identity<T>(x: T): T { x }
```

### Lambdas

```ambient
(a) => a + 1                    // Single parameter
(a, b) => a + b                 // Multiple parameters
() => 42                        // No parameters
(a) => { let x = a + 1; x * 2 } // Multi-line
```

### String Interpolation

```ambient
let greeting = "Hello, ${name}!";
let sum = "Sum: ${to_string(a + b)}";
```

---

## Nominal Types

A `unique(<uuid>) struct` declaration gives a type its own identity, distinct
from any structurally identical type. That identity *is* the UUID:

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: string }
```

UUID literals are written in canonical `8-4-4-4-12` form and **must be
uppercase** (`0-9`, `A-F`). Uppercase is a lexical requirement, not a
convention: the lexer recognizes an uppercase UUID as a single token, which
keeps it unambiguous against identifiers, numbers, and future lowercase `0x`
hex literals. A lowercase or malformed UUID is a syntax error. The stored
value is canonicalized to lowercase for content addressing and display; only
the *source syntax* is uppercase.

## Nominal Enums

Enums are nominal too, and their `unique(<uuid>)` prefix is **mandatory** —
a bare `enum` is a compile error. This is deliberate: an enum's identity is
its UUID, not its shape or its name, so two structurally identical enums (or
two enums with the same name in different packages) are distinct,
non-interchangeable types.

```ambient
unique(E1B2C3D4-0000-0000-0000-000000000001) enum Shape {
  Circle(number), Square(number), Dot
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
Content-Addressing](#dispatch-coherence-and-content-addressing) — so a
same-named enum elsewhere can never claim them.

`Option<T>` and `Result<T, E>` are nominal on the same footing: they carry
fixed reserved UUIDs (`OPTION_UUID`/`RESULT_UUID`), so they are as distinct
and non-interchangeable as any declared enum. Their canonical declarations
are ordinary Ambient source — `pub unique(…FFFF0001) enum Option<T>` in
`core_lib/option.ab`, likewise `Result` in `core_lib/result.ab` — alongside
their combinators and predicates, exposed as inherent methods (`map`,
`and_then`, `is_some`, `unwrap_or`, `is_ok`, …). What makes them special is
the *prelude*: the engine that
builds the type checker's prelude has no parser (the parser depends on the
engine, not the other way round), so it registers the same two enums from a
Rust-side spec (`PRELUDE_ENUMS`) into every module's scope — which is why
`Some`, `None`, `Ok`, and `Err` need no import anywhere. The spec and the
source declaration cannot drift: a declaration that claims a reserved uuid
must match the spec's name, type parameters, and variant layout exactly
(`validate_reserved_declaration`), so the core sources fail the build if
they diverge — and no other module can hijack a reserved identity for a
different shape.

## Traits

Traits define shared behavior for types. Only nominal types can implement
traits. (Types also take methods directly, without a trait — see
[Inherent Impls](#inherent-impls).)

### Defining Traits

```ambient
trait Show {
  fn show(self): string;
}

trait Add {
  fn add(self, other: Self): Self;
}

trait Eq {
  fn eq(self, other: Self): bool;
}
```

The `Self` type refers to the implementing type.

### Implementing Traits

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Money { cents: number }

impl Show for Money {
  fn show(self): string {
    "$" + to_string(self.cents / 100)
  }
}

impl Add for Money {
  fn add(self, other: Money): Money {
    Money { cents: self.cents + other.cents }
  }
}

impl Eq for Money {
  fn eq(self, other: Money): bool {
    self.cents == other.cents
  }
}
```

### Method Calls

Methods are called using dot notation:

```ambient
let m = Money { cents: 1500 };
let s = m.show();           // "$15"

let a = Money { cents: 100 };
let b = Money { cents: 50 };
let c = a.add(b);           // Money { cents: 150 }
```

### Operator Overloading

Standard operators dispatch to trait methods for nominal types:

| Operator | Trait | Method                         |
| -------- | ----- | ------------------------------ |
| `+`      | `Add` | `add(self, other: Self): Self` |
| `-`      | `Sub` | `sub(self, other: Self): Self` |
| `*`      | `Mul` | `mul(self, other: Self): Self` |
| `/`      | `Div` | `div(self, other: Self): Self` |
| `%`      | `Mod` | `rem(self, other: Self): Self` |
| `==`     | `Eq`  | `eq(self, other: Self): bool`  |
| `!=`     | `Eq`  | `eq` (negated)                 |

```ambient
let a = Money { cents: 100 };
let b = Money { cents: 50 };
let c = a + b;              // Calls a.add(b)
let equal = a == b;         // Calls a.eq(b)
```

For primitive types (`number`, `bool`, `string`), operators use built-in implementations.

Trait method signatures carry no `with` clause, so **trait impl method
bodies must be pure** — the checker enforces it. An effectful method
would otherwise launder abilities past callers invisibly: dot dispatch
and operator overloading consult only the trait signature. (Effectful
shared behavior belongs in ordinary functions, or awaits effect-carrying
trait signatures as a language extension.)

### Associated Functions

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

### Inherent Impls

An `impl` block without a trait attaches methods directly to a type. This
is how a type grows an API that isn't shared behavior — no trait ceremony
required:

```ambient
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct Money { cents: number }

impl Money {
  fn double(self): Money {
    Money { cents: self.cents * 2 }
  }
  fn from_dollars(d: number): Money {   // no self: associated function,
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

### Prelude Traits

The operator traits (`Add`, `Sub`, `Mul`, `Div`, `Mod`, `Eq`, `Ord`) plus
`Default` are part of the prelude: they are always in scope, and
implementing an operator trait enables the corresponding operator.
`core::traits` mirrors their definitions for documentation. A module that
declares its own trait with the same name shadows the prelude entry.

`Default` supplies a canonical value for a type via the associated function
`default(): Self` (see [Associated Functions](#associated-functions)):

```ambient
trait Default {
  fn default(): Self;
}
```

The `Ord` trait is used for comparison operators:

```ambient
trait Ord {
  fn cmp(self, other: Self): number;  // -1, 0, or 1
}
```

Comparison operators adapt the trait method's result: `!=` negates
`Eq.eq`, and `<`, `<=`, `>`, `>=` compare `Ord.cmp`'s result against 0.

### Dispatch, Coherence, and Content-Addressing

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

#### Why this survives live upgrade

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
owned by the planned process model (state is re-established through a
well-defined boundary on upgrade), not by the impl system. Coherence
within one build plus hash-pinned dispatch across builds is the whole
mechanical story.

---

## Abilities

Abilities are the mechanism for controlled side effects.

### Ability Identity

An ability is identified by the **blake3 hash of its canonical
interface**: its name plus the ordered list of method names and
canonicalized signatures (type variables numbered by first occurrence,
so `<T>(T) -> U` encodes identically everywhere). Every ability hashes
through the same scheme (`ambient-core/src/canonical.rs`) — the platform
builtins are ordinary in-language declarations, not a special case.

This is the same trick the language plays for functions, and it is what
makes abilities portable: compiled bytecode references abilities by hash
through the constant pool, so a function's content hash commits to the
exact interface of every ability it performs, and two engines that
compute the same ability hash agree on what performing it means. Change
a method name, an argument type, or the ability's name and it is a
different ability.

### Declaring Abilities

Modules declare abilities with `ability`; the type checker resolves the
signatures, computes the interface hash, and the declaration behaves
exactly like a builtin from then on (effect rows, `handle`, handler
values, generic methods):

```ambient
ability FileSystem {
  fn read(path: string): string;
  fn write(path: string, content: string): ();
  fn exists(path: string): bool;
}

ability Picker {
  fn pick<T>(a: T, b: T): T;   // generic methods instantiate per call
}

// Abilities can depend on other abilities: performing Log also
// requires Stdio in the effect row.
ability Log with Stdio {
  fn info(message: string): ();
}
```

The platform abilities (Stdio, FileSystem, Network, ...) are themselves plain
`ability` declarations — see "The platform module" below. User abilities
are handled in-language (`handle` blocks or handler values); a performed
ability with no handler in scope — in-language or host — is a runtime
error. Abilities import across modules like any other item: `use
pkg::b::SomeAbility;` (and `use platform::Network;`) brings the ability into
scope under its bare name, and every ability is also reachable fully
qualified with no import (`with pkg::b::SomeAbility`,
`pkg::b::SomeAbility::method!(…)`) — the same rule as every other item.
Current limit: the REPL does not yet register `platform` as a module, so
bare `use platform::…` there is a follow-up.

### Using Abilities

```ambient
// Perform with ! (FileSystem here is the module-local declaration above;
// the platform ability would be platform::FileSystem::read!)
let content = FileSystem::read!("file.txt");
```

Every module's abilities are in scope *fully-qualified* (`platform::Stdio`
or `pkg::effects::Counter` in performs, `with` clauses, effect-row
annotations, handler arms, and sandbox clauses) with no `use` — the same
rule as every other item. To drop the prefix, import the ability:
`use platform::Stdio;` then `with Stdio` and `Stdio::out!(...)` work
bare thereafter. A bare `Stdio` that was *never* imported (and is not a
local declaration) is a type error — the diagnostic suggests qualifying with
`platform::` or adding the `use`. A local `ability Stdio` shadows an
imported one under the bare name; the platform one stays reachable
qualified. The builtin `Exception` is always bare and may not be spelled
with a namespace.

### Ability Syntax in Type Signatures

```ambient
// read_config uses the module-local FileSystem declared above.
fn read_config(path: Path): Config
  with FileSystem
{
  let content = FileSystem::read!(path);
  parse_config(content)
}

// Multiple abilities; platform abilities keep their namespace in
// effect rows, and mix freely with local declarations like Log.
fn fetch_and_log(url: Url): Response
  with platform::Network, Log
{ ... }

// No abilities (pure function)
fn add(x: number, y: number): number { x + y }
```

### Ability Polymorphism

```ambient
// E is an ability variable
fn map<T, U, E!>(list: List<T>, f: (T) -> U with E): List<U>
  with E
{ ... }

// Partial annotation with _
pub fn transform(x: Input): Output
  with FileSystem, _
{ ... }
```

### Handling Abilities

```ambient
fn run(): () {
  handle read_config("config.toml") {
    FileSystem::read(path) => {
      let content = "contents from anywhere";
      resume(content)
    }
  }
}
```

Handler arms are fully typed against the ability's declared interface:

- Arm parameters take the method's declared parameter types
  (`FileSystem::read(path)` binds `path: string`).
- `resume(v)` feeds the continuation, so `v` must have the method's
  return type; the `resume(...)` expression itself has the handle
  expression's result type.
- An arm body's own effects (performs outside the handled body) count
  against the enclosing function, like any other code.
- Exception is special: `throw` returns `!`, and the host raises it at
  arbitrary perform sites (see "Host failures are catchable exceptions"),
  so the value passed to `resume` in an Exception arm is deliberately
  unconstrained — it substitutes for the *failing call's* result, whose
  type is unknowable at the arm.

### Handlers as Values

```ambient
let mock_fs: Handler<FileSystem> = {
  read(path) => resume("mock content"),
  write(path, content) => resume(()),
  exists(path) => resume(true),
};

// Use handler values
handle unit_test() with mock_fs, mock_network {}

// Override specific methods
handle unit_test() with mock_fs {
  FileSystem::read(path) => resume("intercepted")
}
```

### Sandboxing

```ambient
sandbox with platform::Log {
  untrusted_code()  // Only the platform Log ability available
}

sandbox {
  pure_untrusted_code()  // No abilities - pure computation only
}
```

The restriction is enforced statically: the body may only *use* the
allowed abilities, checked at compile time (including through calls to
functions defined elsewhere in the module). The sandbox installs no
handlers — allowed abilities still execute against the enclosing
context's handlers, so the body's effects count against the enclosing
function like any other code.

### The platform module and host bindings

Builtin abilities are not defined in engine code. The engine's only
native ability is `Exception` (part of the language). Everything else —
Stdio, Time, Random, Log, FileSystem, Network, Process, Execute — is declared once, in
Ambient source, in the **platform bindings interface**
(`crates/ambient-platform/src/platform.ab`).

`platform` is a first-class importable module root, resolved through the
same `ModuleRegistry` machinery as `core::`/`pkg::` (it is a *contextual*
keyword — recognized only in use-prefix position, so it still lexes as a
plain identifier in an ability head like `with platform::Network`). Its
abilities are always in scope fully-qualified (`platform::FileSystem::read!`,
`with platform::FileSystem`, `platform::FileSystem::read(path) => ...`,
`sandbox with platform::Log`) with no `use`, and importable by name
(`use platform::FileSystem;`) to use bare — exactly like `core::` items.
Because ability identity is the content-addressed interface hash, a bare
imported `FileSystem` and a qualified `platform::FileSystem` share one
`AbilityId`, so handlers, effect rows, and linking unify with no special
casing.

The declarations in `platform.ab` are `pub` for the same reason core
exports are: the registry only imports public symbols, so `use
platform::FileSystem;` requires `pub ability FileSystem`. Visibility gates
*only* the bare-import path — fully-qualified use is seeded independently.

The engine seeds the namespaced `platform::` abilities from the registered
`platform` module during type checking (`seed_namespaced_platform_dynamics`),
and the general cross-module bridge (`build_import_env`) registers an
imported ability as a bare dynamic — the same code path any
`use pkg::b::SomeAbility;` takes.

An embedder still wires the **host binding** half:

1. Parse the declarations and resolve them
   (`resolve_ability_declarations`) into content-addressed interfaces, and
   register the `platform` module in the registry
   (`register_declaration_module`) so the naming layer resolves it.
2. Pass the resolved interfaces in `CompileOptions::prelude_abilities` for
   compilation. Type checking no longer needs an embedder ability resolver:
   performs resolve against the seeded `platform` module, the same path
   user-declared abilities take.
3. Bind host handlers **by method name** against the resolved
   interfaces (`AbilityInterface`: identity plus name→method-id map) via
   `vm.register_host_handler(id, method_id, handler)`.

`ambient-platform` is one such embedder, packaged as a library: it ships
`platform.ab` plus native handler sets (std::fs, TCP via tokio, ...) and
registration functions (`register_defaults`, `register_network`,
`register_execute`). The engine crate does not depend on it — another
crate can use the engine the same way with entirely different
declarations and bindings. Because handlers bind by name at wiring time,
editing a declaration re-keys everything consistently; there is no
second copy of the interface to fall out of sync.

---

## Concurrency

All IO is blocking. There is no `Async` ability and no async/await-style
primitives — this is intentional. A perform like `platform::Network::receive!`
simply blocks the calling code until the host handler returns.

Concurrency comes from the Erlang-inspired **process model** (see
`ref/processes.md`): named reducer processes with isolated state,
communicating by message passing through the `platform::Process`
ability. Each process runs on its own thread with its own VM, so a
blocked process blocks only itself. This design is chosen for
live-upgrade correctness — hot code replacement needs a well-defined
unit of state to hand off, and a process mailbox/reducer boundary
provides exactly that. `ambient dev` upgrades a running process tree by
re-running the entry function as a reconciliation pass: content hashes
decide which processes' code changed; changed reducers are swapped at
their next message boundary, keeping their state.

---

## Error Handling

Errors are abilities. `Exception::throw!` raises; the nearest enclosing
`handle` block for Exception catches. A handler arm's value becomes the
handle expression's value, and execution continues after the handle
expression (catch-and-continue). The optional `else` clause transforms
the body's value on _normal_ completion only; arms bypass it.

```ambient
ability Exception {
  fn throw(error: string): !;  // ! = never returns normally
}

fn parse_int(s: string): number with Exception {
  match try_parse(s) {
    Some(n) => n,
    None => Exception::throw!("not a number"),
  }
}

// Handling exceptions
fn safe_parse(s: string): Option<number> {
  handle parse_int(s) {
    Exception::throw(e) => None
    else { (result) => Some(result) }
  }
}
```

An uncaught throw halts the program with `uncaught exception: <value>`,
carrying the actual thrown value.

### Host failures are catchable exceptions

Fallible host operations (file not found, connection refused, ...) do not
return `Result` values and do not kill the VM: the host handler raises
`Exception.throw(message)` _at the perform site_. The calling program
catches it like any in-language throw. Because the Exception handler
receives the continuation of the failed call, it can even `resume` with a
substitute value, and the IO caller continues as if the operation had
succeeded:

```ambient
fn fetch_or_default(): number with platform::Network {
  handle platform::Network::connect!(("10.0.0.1", 9)) {
    Exception::throw(msg) => resume(0 - 1)  // substitute connection id
  }
}
```

Engine-level faults (stack overflow, type errors in bytecode, arity
mismatches) remain fatal `VmError`s - they indicate bugs, not conditions
programs should handle.

Current limits: `Exception` is not generic yet (`throw` takes a string;
`Exception<E>` with an error trait bound is the planned evolution), and
`!` (never) does not yet unify with other types, so `throw` works in
statement position but not as the value of a typed expression.

### Option/Result vs exceptions

`Option` and `Result` are ordinary data types for _domain modeling_: a
lookup that may find nothing returns `Option`, a parser that produces a
structured error returns `Result`. They are values you match on.

_Operational failure_ - the file was deleted, the peer hung up - is not
data the caller asked for; it is an interruption of an effect, and it
travels through the effect system as an Exception. No builtin ability
returns `Result` to signal failure. This keeps IO signatures honest
(`FileSystem.read` returns `string`, not `Result<string, _>`) while `handle`
gives callers strictly more power than matching: they can substitute a
fallback for the failing call and continue, not just observe the error
after the fact.

---

## Type Inference Rules

1. **Public functions**: Abilities must be declared (`with ...`); no clause
   means pure. Using an undeclared ability is a type error. Return types
   must be declared.
2. **Private functions**: Abilities are inferred from the body when no
   `with` clause is given (a clause, if present, is enforced). Inferred
   abilities propagate to callers and count against the declarations of any
   public function that transitively reaches them.
3. **Local variables**: Always inferred
4. **Lambdas**: Parameter types inferred from context; the lambda's ability
   set is inferred from its body and carried on its function type
5. **Effect propagation**: Calling a function requires the caller to provide
   (declare, infer, or handle) the callee's abilities

---

## Core Library

### Core Abilities

The authoritative declarations live in
`crates/ambient-platform/src/platform.ab`; excerpts:

```ambient
ability Time {
  fn now(): number;               // ms since the Unix epoch
  fn wait(duration: number): (); // ms
}

ability Random {
  fn seed(): number;              // 0.0 to 1.0
  fn in_range(max: number): number;
}

ability Stdio {
  fn out(message: string): ();   // write a line to stdout
  fn err(message: string): ();   // write a line to stderr
  fn read(): string;             // read a line from stdin
}

// Log is emitted through Stdio, so it declares the dependency: performing
// Log requires Stdio in the effect row, and a handler for Stdio captures
// log lines.
ability Log with platform::Stdio {
  fn debug(message: string): ();
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}

ability FileSystem {
  fn read(path: string): string;              // UTF-8 text
  fn write(path: string, content: string): ();  // create/truncate
  fn read_binary(path: string): Binary;
  fn write_binary(path: string, data: Binary): ();
  fn exists(path: string): bool;              // infallible
  fn list(path: string): List<string>;        // sorted entry names
  fn remove(path: string): ();                // file or empty directory
  fn create_dir(path: string): ();            // mkdir -p
}

ability Process {
  fn spawn<I, H>(name: string, init: I, handler: H): number;
  fn send<M>(pid: number, msg: M): ();
  fn send_named<M>(name: string, msg: M): ();
  fn self_pid(): number;              // 0 outside any process
  fn whereis(name: string): number;   // 0 if no such name
  fn exit(): ();                      // stop after the current reduction
}
```

`Process` is the surface of the process model (`ref/processes.md`):
named reducer processes with isolated state, message passing, flat
supervision, and reconciliation-based live upgrade under `ambient dev`.

FileSystem failures (missing files, permission errors, invalid UTF-8) raise
catchable `Exception`s, recoverable with
`handle ... { Exception.throw(msg) => ... }`. Only `exists` is
infallible: it returns `false` when the path can't be inspected.

### Standard Functions

Option, Result, and List expose their combinators and predicates as inherent
methods — the canonical core API — so pipelines read receiver-first
(head/get/first/last return Option — no sentinel values):

```ambient
[1, 2, 3].filter((x) => x % 2 == 1).map((x) => x * 10).fold(0, (a, x) => a + x)
Some(20).map((v) => v * 2).unwrap_or(0)
Ok(5).map((v) => v + 1).ok().is_some()
```

The method names, by type:

```ambient
// Collections
List::{map, filter, fold, any, all, sum, get, head, tail, first, last,
       length, is_empty, reverse, sort, slice, append, concat}

// Options
Option::{map, and_then, or_else, is_some, is_none, unwrap_or}

// Results
Result::{map, map_err, and_then, ok, is_ok, is_err, unwrap_or}
```

Two combinators have no method form and stay qualified module functions:
`core::collections::List::range(start, end)` (no receiver) and `core::Option::flatten(opt)`
(its receiver would be `Option<Option<U>>`, inexpressible in an
`impl<T> Option<T>` block). Strings and conversions remain module functions:

```ambient
// Strings
String::split, String::join, String::trim, String::contains, String::length

// Conversion (parsers return Option)
to_string, parse_number, parse_bool
```

---

## Architecture

```
Source (.ab) → Parser (CST → AST) → Type Checker → Compiler → Bytecode
                                                         ↓
                                            Content-Addressed Store
                                                         ↓
                                                   Bytecode VM
```

### The shared analysis layer

`crates/ambient-analysis` composes the parser and engine into one
parse → check → diagnose pipeline, and it is the *only* place that
decides what is an error. `ambient check` and the language server are
both renderers over it: same package loading, same `ModuleRegistry`,
same diagnostic text and spans, byte for byte (pinned by
`crates/ambient-lsp/tests/parity.rs`). Behavior that must differ
between the compiler and the editor is expressed inside the analysis
crate, where it is visible and testable — never re-implemented in a
frontend.

Editor buffers are routinely mid-edit, so analysis parses with
recovery (`ambient_parser::parse_recovering`): items that fail to
parse are dropped, everything else is analyzed, and a broken file
still contributes its parseable exports to the rest of the package.
Type errors are computed on the partial module (they feed hover and
completion) but reporting suppresses them while parse errors exist —
a module with missing items produces cascading nonsense. The compiler
pipeline (`build_package`) keeps fail-fast parsing.

Cross-module name resolution — for the checker, for `use` handling,
for go-to-definition and completions alike — is the engine's
`ModuleRegistry` (`resolve_imports`, `lookup_symbol`,
span-and-doc-carrying `ExportInfo`). The LSP holds no parallel index.

### Bytecode VM

Stack-based VM with:

- Value stack for operands
- Call stack for function frames
- Continuation stack for ability handlers

### Content-Addressing

A function's hash is the blake3 of its **canonical object encoding**
(`crates/ambient-engine/src/object.rs`). The encoding covers the bytecode,
the constant pool (with call sites resolved to the final hashes of their
callees, and abilities resolved to their interface hashes), arity/locals
metadata, and the dependency list — so the hash pins the implementation,
every transitive dependency, _and_ the exact interface of every ability
the function performs. One encoding serves as
the unit of hashing, storage, and network transfer, which makes every object
self-verifying: re-hash the bytes and compare.

Two kinds of objects:

- **Plain** — one non-recursive function. `hash = blake3(encoding)`.
  Names are _not_ part of the encoding: renaming never changes the hash.
- **Group** — one strongly connected component of mutually (or self-)
  recursive functions, stored as a single unit. References between members
  are encoded as member indices, which breaks the circularity that makes
  recursive functions otherwise un-hashable. Member hashes derive from the
  group: the group hash itself for singletons, else
  `blake3("ambient/member/v1" ‖ group_hash ‖ index)`. Named members sort by
  name (names of cycle members are part of the group identity — they are the
  only way to distinguish members); lambdas order by first reference.

Invariants (pinned by `crates/ambient-parser/tests/content_addressing.rs`):

- Compiling the same source twice yields identical hashes.
- Declaration order and unrelated declarations never affect a hash
  (including unrelated lambdas vs. recursive groups).
- Changing a function's body, or any transitive dependency, changes its hash.
- Renaming a non-recursive function never changes its hash.
- Every compiled function is reproducible byte-for-byte from its object.
- Impl methods are ordinary functions — trait methods named
  `<type-uuid>::<Trait>::<method>`, inherent methods
  `<type-identity>::<method>` — and obey all of the above.

### The Store

The store exists in three forms, all sharing the canonical object encoding:

1. **In-memory** (`store.rs`) — runtime view; materialized functions plus
   their objects. Used by the VM and the Execute ability.
2. **On disk** (`disk_store.rs`) — persisted per package at
   `<pkg>/.ambient/store/`, git-style:

   ```
   .ambient/store/
     format                    version marker
     names                     "<hex-hash> <name>" per binding
     objects/<2hex>/<62hex>    one canonical object per file
   ```

   An object file's path is the blake3 of its bytes, so every read
   self-verifies and corruption is always detected. Objects are immutable;
   writes are atomic renames, so no locking is needed. `ambient run`
   persists every build. Inspect with `ambient store`
   (stats/ls/show/deps/verify/gc) — `show` includes a full disassembly.

3. **Packs** (`"ABPK"`) — a batch of objects for transfer: the wire format
   of the Execute ability (remote code shipping) and the content of
   `.ambient` artifact files (which add name bindings and an entry point).
   Receivers recompute all hashes from object bytes; a tampered pack cannot
   smuggle code under a false hash.

### Delimited Continuations

Abilities implemented using single-shot delimited continuations.

- Continuation can be resumed at most once (runtime error on double resume)
- Supports: exceptions, I/O, state
- Does not support: backtracking, multi-shot operators

A handle expression compiles its body into a thunk closure whose call
frame delimits the handled computation. A perform captures the frames,
stack segment, and handler entries above that boundary into the
continuation (all offsets relative, so resume rebases them anywhere),
then runs the handler arm in place of the thunk call: a non-resuming arm
returns straight to the handle expression's completion point. Handlers
are _deep_: resuming re-installs the captured handlers, so a body that
performs repeatedly fires the same arm each time. Handler arms are
closures and may capture from the enclosing scope.

### Execution Model

Blocking IO on plain threads:

1. A VM is single-threaded; ability handlers block until the host
   operation completes
2. Concurrency is processes: each process owns a thread and a VM, and
   the process runtime routes messages between them (see Concurrency
   and `ref/processes.md`)

---

## Remote Execution

```
Client                          Server
  |-- Execute(hash, args) ------->|
  |<-- NeedDeps([hash1, hash2]) --|  (if missing)
  |-- Provide([fn1, fn2]) ------->|
  |<-- Result(value) -------------|
```

Remote execution servers are written in Ambient itself on the `Network`
(TCP) and `Execute` (run-by-hash) abilities; the message framing above is
a convention of the example programs, not engine code. Code ships as
canonical object packs — receivers recompute every hash from the bytes.

Executed code runs in an **isolated VM**, and the remote must provide all
ability handlers — nothing proxies back to the caller. Effects reach it
two ways:

- **Host grants** (`ExecuteConfig::grants`): the executing host decides
  which host handlers each isolated VM gets. The CLI grants Stdio and
  Log — shipped code can print/log on the executing host but has no
  Network, Time, Random, or recursive Execute. Performing an ungranted
  ability is a hard unhandled-ability error. This is the wasm-style
  split: the engine is pure; hosts bind effectful capabilities to pure
  ability interfaces, and different embeddings grant different sets.
- **Shipped handlers** (`Execute.run_with(hash, arg, handler)`): a
  first-class handler value travels with the call — its methods are
  content-addressed functions, shipped in packs like any code — and is
  installed at the base of the isolated VM. Ability hashes make this
  sound: handler and perform match only if both sides computed the same
  interface hash. `core::protocol::handler_methods(h)` exposes a handler's
  method hashes so clients can ship its code.

Values cross via `core::protocol::serialize_value`/`deserialize_value`
(bincode of the wire-safe subset: primitives, tuples/lists/records,
enums, function refs by hash, handler values by hash table). Closures,
continuations, maps/sets, and modules do not cross; serializing one is a
runtime error, never a silent `()`.

---

## CLI

```bash
ambient init my_project    # Scaffold a new package
ambient run <pkg-dir>      # Compile and run a package (or a .ambient artifact)
ambient check foo.ab       # Type-check only
ambient compile foo.ab     # Compile to a foo.ambient artifact pack
ambient ast foo.ab         # Dump the parsed AST
ambient store stats        # Inspect the package store (also: ls, show,
                           #   deps, verify, gc; show disassembles)
ambient repl               # Interactive REPL
ambient dev <pkg>          # Live-upgrade development: watches sources and
                           #   hot-swaps changed processes, keeping state
                           #   (falls back to rerun-on-change for programs
                           #   that spawn no processes)
ambient lsp                # Start the language server
```

Remote execution servers are written in Ambient itself using the `Network`
and `Execute` abilities (see `examples/remote_server`); there is no
dedicated `serve` command.

---

## Example Programs

### Hello World

```ambient
pub fn run(): () with platform::Stdio {
  platform::Stdio::out!("Hello, world!");
}
```

### Factorial

```ambient
fn factorial(n: number): number {
  if n <= 1 { 1 } else { n * factorial(n - 1) }
}
```

### Vector Math with Traits

```ambient
// Add and Eq come from the prelude; implementing them enables + and ==.
unique(A1B2C3D4-0000-0000-0000-000000000001) struct Vec2 { x: number, y: number }

impl Add for Vec2 {
  fn add(self, other: Vec2): Vec2 {
    Vec2 { x: self.x + other.x, y: self.y + other.y }
  }
}

impl Eq for Vec2 {
  fn eq(self, other: Vec2): bool {
    self.x == other.x && self.y == other.y
  }
}

fn run(): bool {
  let a = Vec2 { x: 1, y: 2 };
  let b = Vec2 { x: 3, y: 4 };
  let c = a + b;              // Vec2 { x: 4, y: 6 }
  c == Vec2 { x: 4, y: 6 }    // true
}
```

### Testing with Mocks

```ambient
let mock_fs: Handler<FileSystem> = {
  read(path) => resume("mock content"),
  write(path, content) => resume(()),
  exists(path) => resume(true),
};

fn test_my_function(): () {
  handle my_function() with mock_fs {}
}
```

---

## Future Work

Roughly in priority order:

- **Grow the standard library as ordinary modules.** The module system now
  treats `core` as a real package (compiled `.ab` modules, whole-module and
  item imports, qualified calls), but the library itself is small
  (list/math/string helpers) and many operations still live only as
  intrinsics. Target roughly the granularity of Go's or Node's standard
  libraries. Generic trait bounds would unlock `contains`/`sort_by`.
- **Cross-module ability imports.** The platform-bindings split is
  done: platform abilities are pure in-language declarations
  (`platform.ab`) embodied by host FFI, the engine crate knows only
  Exception, and embedders wire declaration hashes to handlers by
  method name. What remains is the general form: exporting an
  `ability` from one user module and importing it in another (exports
  carry the kind but consumers don't hydrate them yet), plus REPL
  registration of user-declared abilities.
- **Process model growth** (see `ref/processes.md` for what exists):
  typed `spawn` via effect-polymorphic ability signatures, process
  linking/monitors, supervision trees, receive timeouts, and
  `Execute`-driven remote deploy passes (live upgrade over the network).
- Generic traits, supertraits, trait bounds (`fn foo<T: Eq>(x: T)`) — only
  if needed; traits exist to support polymorphic operators
- Incremental compilation backed by the persisted store
- WASM target
- Workspace mechanism (multi-package local development) — lands before the
  package manager
- Package manager with a shared cache (stores are rsync-friendly by
  construction)
- Match exhaustiveness checking (a failing final variant arm is a
  runtime error today)
- Name-aware type rendering: ability sets display as content hashes in
  compiler errors and IDE hover; rendering needs a resolver at every
  display site (one shared engine renderer, consumed by CLI and LSP)
- Type unions (`A | B`)
- Mutability — some form of first-class mutable state.

## Non-Goals

Deliberately out of scope — recorded here so they stop resurfacing as future
work:

- **Affine/linear types.** Too much type-system complexity for a scripting
  language, and a poor fit for its ergonomics — including as a route to
  mutability, which stays an open goal above via some other mechanism.
- **Compiling Ambient programs to WASM.** The WASM target above compiles the
  _interpreter_; user programs are never lowered to WASM.
