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
use pkg.utils.{a, b};   // Item import: a and b as bare names
use self.utils;         // Whole-module import: call utils.helper(...)
use core.list;          // Core modules import the same way: list.map(...)
use core.list.{map};    // ... or by item
```

Core modules (`core.list`, `core.math`, `core.string`) are also always in
scope fully qualified with no import: `core.list.map([1], f)`. They are
ordinary Ambient modules — compiled, content-addressed, and stored exactly
like user code (see `crates/ambient-engine/src/core_lib/`). Beneath them
sits a fixed set of *intrinsics* (`core.math.sqrt`, `core.list.length`,
`core.string.concat`, ...) that compile to dedicated opcodes; intrinsics
take precedence over compiled functions at the same path. `core` is a
keyword, so user modules can never collide with the standard library.

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

Traits define shared behavior for types. Only nominal types can implement traits.

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

| Operator | Trait | Method |
|----------|-------|--------|
| `+` | `Add` | `add(self, other: Self): Self` |
| `-` | `Sub` | `sub(self, other: Self): Self` |
| `*` | `Mul` | `mul(self, other: Self): Self` |
| `/` | `Div` | `div(self, other: Self): Self` |
| `%` | `Mod` | `rem(self, other: Self): Self` |
| `==` | `Eq` | `eq(self, other: Self): bool` |
| `!=` | `Eq` | `eq` (negated) |

```ambient
let a = Money { cents: 100 };
let b = Money { cents: 50 };
let c = a + b;              // Calls a.add(b)
let equal = a == b;         // Calls a.eq(b)
```

For primitive types (`number`, `bool`, `string`), operators use built-in implementations.

### Prelude Traits

The operator traits (`Add`, `Sub`, `Mul`, `Div`, `Mod`, `Eq`, `Ord`) are
part of the prelude: they are always in scope, and implementing one enables
the corresponding operator. `core.traits` mirrors their definitions for
documentation. A module that declares its own trait with the same name
shadows the prelude entry.

The `Ord` trait is used for comparison operators:

```ambient
trait Ord {
  fn cmp(self, other: Self): number;  // -1, 0, or 1
}
```

Comparison operators adapt the trait method's result: `!=` negates
`Eq.eq`, and `<`, `<=`, `>`, `>=` compare `Ord.cmp`'s result against 0.

### Trait Dispatch and Content-Addressing

Method calls dispatch statically: the receiver's concrete nominal type is
known during type checking, which resolves the call to a canonical method
symbol `<type-uuid>::<Trait>::<method>`. Impl methods compile as ordinary
named functions under that symbol, so they are content-addressed exactly
like any other function (hash = bytecode + constants + dependency hashes),
and call sites link against the content hash. There is no runtime trait
registry and no dynamic dispatch.

Traits and impls declared anywhere in a package are visible to every module
in it (impl coherence is global per package).

---

## Abilities

Abilities are the mechanism for controlled side effects.

### Ability Identity

An ability is identified by the **blake3 hash of its canonical
interface**: its name plus the ordered list of method names and
canonicalized signatures (type variables numbered by first occurrence,
so `<T>(T) -> U` encodes identically everywhere). Builtin abilities and
in-language declarations hash through the same scheme
(`ambient-core/src/canonical.rs`).

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
ability Filesystem {
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

User abilities are handled in-language (`handle` blocks or handler
values); the host only provides handlers for builtins. A performed user
ability with no handler in scope is a runtime error. Current limits:
declarations are visible in the declaring module (no cross-module
ability imports yet), and the REPL does not register them.

### Using Abilities

```ambient
// Perform with !
let content = Filesystem.read!("file.txt");
```

### Ability Syntax in Type Signatures

```ambient
fn read_config(path: Path): Config
  with Filesystem
{
  let content = Filesystem.read!(path);
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
  with Filesystem, _
{ ... }
```

### Handling Abilities

```ambient
fn run(): () {
  handle read_config("config.toml") {
    Filesystem.read(path) => {
      let content = host_read_file(path);
      resume(content)
    }
  }
}
```

### Handlers as Values

```ambient
let mock_fs: Handler<Filesystem> = {
  read(path) => resume("mock content"),
  write(path, content) => resume(()),
  exists(path) => resume(true),
};

// Use handler values
handle unit_test() with mock_fs, mock_network {}

// Override specific methods
handle unit_test() with mock_fs {
  Filesystem.read(path) => resume("intercepted")
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

---

## Concurrency

All IO is blocking. There is no `Async` ability and no async/await-style
primitives — this is intentional. A perform like `runtime.Network.receive!`
simply blocks the calling code until the host handler returns.

The planned direction is an Erlang-inspired process model: lightweight
green processes with isolated state, communicating by message passing,
organized under supervisors. This design is chosen for live-upgrade
correctness — hot code replacement needs a well-defined unit of state to
hand off, and a process mailbox/state boundary provides exactly that.
Blocking IO composes naturally with that model (a blocked process yields
its scheduler thread), whereas async/await would thread a second
concurrency model through the language.

This process model is future work: it is not yet designed in detail and
nothing in the current runtime implements it.

---

## Error Handling

Errors are abilities:

```ambient
ability Exception<E> {
  fn throw(error: E): !;  // ! = never returns normally
}

fn parse_int(s: string): number
  with Exception<ParseError>
{
  match try_parse(s) {
    Some(n) => n,
    None => Exception.throw!(ParseError { input: s }),
  }
}

// Handling exceptions
fn safe_parse(s: string): Option<number> {
  handle parse_int(s) {
    Exception.throw(e) => None
  } else {
    (result) => Some(result)
  }
}
```

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

```ambient
ability Time {
  fn now(): Timestamp;
  fn wait(duration: Duration): ();
}

ability Random {
  fn seed(): number;              // 0.0 to 1.0
  fn in_range(range: Range): number;
}

ability Console {
  fn print(message: string): ();
}

ability Log with Console {
  fn debug(message: string): ();
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}
```

### Standard Functions

```ambient
// Collections
List.map, List.filter, List.fold, List.concat, List.length, List.head, List.tail

// Options
Option.map, Option.unwrap_or, Option.and_then

// Results
Result.map, Result.map_err, Result.and_then

// Strings
String.split, String.join, String.trim, String.contains, String.length

// Conversion
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
every transitive dependency, *and* the exact interface of every ability
the function performs. One encoding serves as
the unit of hashing, storage, and network transfer, which makes every object
self-verifying: re-hash the bytes and compare.

Two kinds of objects:

- **Plain** — one non-recursive function. `hash = blake3(encoding)`.
  Names are *not* part of the encoding: renaming never changes the hash.
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
- Trait impl methods are ordinary functions named
  `<type-uuid>::<Trait>::<method>` and obey all of the above.

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

### Execution Model

Single-threaded with blocking IO:
1. User code runs on a single thread
2. Ability handlers block until the host operation completes
3. Concurrency is future work: an Erlang-style process model (see Concurrency)

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
  interface hash. `core.protocol.handler_methods(h)` exposes a handler's
  method hashes so clients can ship its code.

Values cross via `core.protocol.serialize_value`/`deserialize_value`
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
ambient dev foo.ab         # Hot reload development
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
  runtime.Console.print!("Hello, world!");
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
let mock_fs: Handler<Filesystem> = {
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
- **Finish the platform-bindings split.** Ability identity is now
  content-addressed, `ability` declarations work in-language, and
  isolated execution takes host-granted capability sets — but builtin
  abilities are still *defined* in Rust descriptor code rather than as
  pure in-language declarations embodied by host FFI (the `io.unix`
  model), and the top-level VM still hardwires the native handler set.
  Cross-module ability imports and a `sandbox` expression fall out of
  the same work.
- Generic traits, supertraits, trait bounds (`fn foo<T: Eq>(x: T)`) — only
  if needed; traits exist to support polymorphic operators
- Incremental compilation backed by the persisted store
- WASM target
- Package manager (stores are rsync-friendly by construction)
- Match exhaustiveness checking (a failing final variant arm is a
  runtime error today)
- Enum variants imported across modules (constructors are currently
  visible to the declaring module plus the prelude)
- Type unions (`A | B`)
- Multi-shot continuations
- Mutable references (`Store<T>` or affine types)
