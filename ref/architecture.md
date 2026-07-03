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
// Constants
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

Modules map 1:1 to files under `src/`. Import prefixes: `pkg` (package
root), `self` (same directory), `super` (parent), `core` (standard
library).

```ambient
use pkg::utils::{a, b};   // Item import: a and b as bare names
use self::utils;         // Whole-module import: call utils::helper(...)
use core::List;          // Core modules import the same way: List::map(...)
use core::List::{map};    // ... or by item
```

Core modules (`core::List`, `core::math`, `core::string`) are also always in
scope fully qualified with no import: `core::List::map([1], f)`. They are
ordinary Ambient modules — compiled, content-addressed, and stored exactly
like user code (see `crates/ambient-engine/src/core_lib/`). Beneath them
sits a fixed set of _intrinsics_ (`core::math::sqrt`, `core::List::length`,
`core::string::concat`, ...) that compile to dedicated opcodes; intrinsics
take precedence over compiled functions at the same path. `core` is a
keyword, so user modules can never collide with the standard library.

A core module that backs a type takes that type's PascalCase name: `List`,
`Option`, and `Result` are the companion modules of the `List<T>`, `Option<T>`,
and `Result<T, E>` types, so `List::map` reads as an associated function of
`List`. Plain namespaces stay lowercase (`math` backs no type; `string` is a
primitive whose `string`→`String` alignment is future work). Types, values, and
modules occupy separate namespaces resolved by syntactic position, so the type
`List` and the module `core::List` coexist without ambiguity.

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

// Enums (tagged unions). Option and Result are built in; their
// constructors (Some, None, Ok, Err) are always in scope.
enum Shape { Circle(number), Square(number), Dot }

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
unique(d098767b-4093-4d5c-ba37-ad92aa7b5d98) type UserId { value: string }

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
unique(d098767b-4093-4d5c-ba37-ad92aa7b5d98) type Money { cents: number }

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
unique(d098767b-4093-4d5c-ba37-ad92aa7b5d98) type Money { cents: number }

impl Money {
  fn double(self): Money {
    Money { cents: self.cents * 2 }
  }
  fn from_dollars(d: number): Money {   // no self: associated function,
    Money { cents: d * 100 }            // called as Money::from_dollars(3)
  }
}
```

Inherent impls are not limited to nominal types. Enums, the built-in
constructors (`Option`, `Result`, `List`, `Map`, `Set`), and the primitives
can all carry methods, and impls may be generic over the target's type
parameters:

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
helpers as methods (see `crates/ambient-engine/src/core_lib/*.ab`), so
combinator chains read left to right:

```ambient
[1, 2, 3].map((x) => x * 10).fold(0, (acc, x) => acc + x)   // 60
Some(20).map((v) => v * 2).unwrap_or(0)                     // 40
```

The module companion functions (`Option::map(opt, f)`, `core::List::map`)
remain — a method call is just the receiver-first spelling of the same
content-addressed function.

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
`<type-identity>::<method>` for inherent methods, where the identity is
the nominal UUID or the built-in/enum head name (`Option::map`). The
segment counts differ, so the two families can never collide. Impl methods
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
// requires Console in the effect row.
ability Log with Console {
  fn info(message: string): ();
}
```

The platform abilities (Console, FileSystem, Network, ...) are themselves plain
`ability` declarations — see "The platform module" below. User abilities
are handled in-language (`handle` blocks or handler values); a performed
ability with no handler in scope — in-language or host — is a runtime
error. Current limits: declarations are visible in the declaring module
(no cross-module ability imports yet), and the REPL does not register
them.

### Using Abilities

```ambient
// Perform with !
let content = FileSystem::read!("file.txt");
```

### Ability Syntax in Type Signatures

```ambient
fn read_config(path: Path): Config
  with FileSystem
{
  let content = FileSystem::read!(path);
  parse_config(content)
}

// Multiple abilities
fn fetch_and_log(url: Url): Response
  with Network, Log
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
sandbox with Log {
  untrusted_code()  // Only Log ability available
}

sandbox {
  pure_untrusted_code()  // No abilities - pure computation only
}
```

### The platform module and host bindings

Builtin abilities are not defined in engine code. The engine's only
native ability is `Exception` (part of the language). Everything else —
Console, Time, Random, Log, FileSystem, Network, Process, Execute — is declared once, in
Ambient source, in the **platform bindings interface**
(`crates/ambient-platform/src/platform.ab`), and performed under the
`platform` namespace: `platform::FileSystem::read!(path)`.

An embedder wires the two halves together:

1. Parse the declarations and resolve them
   (`resolve_ability_declarations`) into content-addressed interfaces.
2. Register them as the `platform` ability prelude on the resolver used
   for type checking and in `CompileOptions::prelude_abilities` for
   compilation. Performs then type-check against the full declared
   signatures — the same path user-declared abilities take.
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
fn fetch_or_default(): number with Network {
  handle platform::Network::connect!("10.0.0.1:9") {
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

ability Console {
  fn print(message: string): ();
  fn eprint(message: string): ();
  fn println(message: string): ();
}

ability Log {
  fn debug(message: string): ();
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}

ability FileSystem {
  fn read(path: string): string;              // UTF-8 text
  fn write(path: string, content: string): ();  // create/truncate
  fn read_bytes(path: string): Bytes;
  fn write_bytes(path: string, data: Bytes): ();
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

```ambient
// Collections
List::map, List::filter, List::fold, List::concat, List::length, List::head, List::tail

// Options
Option::map, Option::unwrap_or, Option::and_then

// Results
Result::map, Result::map_err, Result::and_then

// Strings
String::split, String::join, String::trim, String::contains, String::length

// Conversion
to_string, parse_number, parse_bool
```

Option, Result, and List expose their combinators as methods too (inherent
impls in the core modules), so pipelines read receiver-first:

```ambient
[1, 2, 3].filter((x) => x % 2 == 1).map((x) => x * 10).fold(0, (a, x) => a + x)
Some(20).map((v) => v * 2).unwrap_or(0)
Ok(5).map((v) => v + 1).ok().is_some()
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
  which host handlers each isolated VM gets. The CLI grants Console and
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
pub fn run(): () with Console {
  platform::Console::print!("Hello, world!");
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
unique(a1b2c3d4-0000-0000-0000-000000000001) type Vec2 { x: number, y: number }

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
- Enum variants imported across modules (constructors are currently
  visible to the declaring module plus the prelude)
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
