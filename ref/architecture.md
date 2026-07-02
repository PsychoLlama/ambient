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

```ambient
use some_library.mod.{a, b.c.d};  // Import specific items
use some_library.utils;            // Import a module
```

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

// Enums (tagged unions)
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }

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

### Declaring Abilities

```ambient
ability Filesystem {
  fn read(path: Path): string;
  fn write(path: Path, content: string): ();
  fn exists(path: Path): bool;
}

ability Console {
  fn print(message: string): ();
}

// Abilities can depend on other abilities (encapsulated)
ability Log with Console {
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}
```

### Using Abilities

```ambient
// Perform immediately with !
let content = Filesystem.read!("file.txt");

// Suspend as value with ~
let read_op = Filesystem.read~("file.txt");  // type: Ability<string, Filesystem!>

// Perform later
let content = read_op!;
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

```ambient
ability Async {
  fn all<T, A!>(ops: List<Ability<T, A!>>): List<T>;
  fn race<T, A!>(ops: List<Ability<T, A!>>): T;
}

// Concurrent execution
let op1 = Network.fetch~(request1);
let op2 = Network.fetch~(request2);
let [r1, r2] = runtime.Async.all!([op1, op2]);

// Race: first to complete wins, others cancelled
let winner = runtime.Async.race!([op1, op2]);
```

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

1. **Functions**: Abilities must be declared (`with ...`); a function using
   an undeclared ability is a type error. (Planned: full ability inference
   for private functions.) Return types of public functions must be declared.
2. **Local variables**: Always inferred
3. **Lambdas**: Parameter types inferred from context; the lambda's ability
   set is inferred from its body and carried on its function type
4. **Effect propagation**: Calling a function requires the caller to provide
   (declare or handle) the callee's abilities

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

Every function identified by hash of:
1. Fully-resolved type signature (including abilities)
2. Implementation (normalized bytecode)
3. Hashes of all called functions

Mutually recursive functions are hashed as a group (SCC).

Invariants (pinned by `crates/ambient-parser/tests/content_addressing.rs`):
- Compiling the same source twice yields identical hashes.
- Declaration order and unrelated declarations never affect a hash.
- Changing a function's body, or any transitive dependency, changes its hash.
- Trait impl methods are ordinary functions named
  `<type-uuid>::<Trait>::<method>` and obey all of the above.

### Delimited Continuations

Abilities implemented using single-shot delimited continuations.
- Continuation can be resumed at most once (runtime error on double resume)
- Supports: exceptions, I/O, state, async
- Does not support: backtracking, multi-shot operators

### Execution Model

Single-threaded with cooperative concurrency:
1. User code runs on single thread
2. Ability handlers may delegate to host thread pool
3. `runtime.Async.all!`/`runtime.Async.race!` suspend operations for concurrent host execution
4. No shared mutable state between concurrent operations

---

## Remote Execution

```
Client                          Server
  |-- Execute(hash, args) ------->|
  |<-- NeedDeps([hash1, hash2]) --|  (if missing)
  |-- Provide([fn1, fn2]) ------->|
  |<-- Result(value) -------------|
```

The remote must provide all ability handlers. No ability proxying back to caller.

---

## CLI

```bash
ambient init my_project    # Scaffold a new package
ambient run <pkg-dir>      # Compile and run a package (or a .ambient file)
ambient check foo.ab       # Type-check only
ambient compile foo.ab     # Compile to foo.ambient
ambient ast foo.ab         # Dump the parsed AST
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

### Concurrent Fetching

```ambient
pub fn fetch_all(urls: List<Url>): List<Response>
  with Network, Async
{
  let ops = List.map(urls, (url) => Network.fetch~(Request { url: url, method: Get }));
  runtime.Async.all!(ops)
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

- Ability inference for private functions (annotations currently required)
- Persisted content-addressed store (store is in-memory; .ambient is JSON)
- Generic traits, supertraits, trait bounds (`fn foo<T: Eq>(x: T)`)
- WASM target
- Embedded target (FFI/crate)
- Package manager
- Type unions (`A | B`)
- Multi-shot continuations
- Mutable references (`Store<T>` or affine types)
- Incremental compilation
