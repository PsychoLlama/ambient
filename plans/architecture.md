# Ambient Language Specification

A content-addressed, ability-based programming language inspired by Unison, Rust, and TypeScript.

## Overview

### Design Philosophy

- **Content-addressed**: Functions are identified by the hash of their implementation and type signature
- **Pure with algebraic abilities**: All side effects are explicit and managed through delimited continuations
- **Remote-capable**: Functions can be serialized and sent to remote VMs for execution or live program replacement
- **File-system synchronized**: Source code lives in files; the filesystem is the source of truth

### Target Environments

1. **CLI interpreter**: Primary target. A standalone interpreter similar to Python or Node that executes compiled `.ab` files
2. **Remote execution / Proxy servers**: Execute functions or replace running programs remotely. Powers HMR in local development and live upgrades in production
3. **Web (WASM)**: Future target. Compile the interpreter to WASM, run programs in browsers
4. **Embedded**: Far future. Bind to the Rust engine over FFI or as a crate for editors, terminals, window managers

---

## Syntax

### Basic Structure

The language is expression-based with C-like syntax. There are no statements, only expressions. Semicolons are required.

```ambient
// Comments use double slashes

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

Use `.` as the delimiter everywhere. Import with `use`:

```ambient
// Import specific items
use some_library.mod.{a, b.c.d};

// Import all from a module
use some_library.utils.*;

// Import all enum variants
use HttpMethod.*;
```

### Types

#### Primitive Types

```ambient
number    // 64-bit float (f64)
string    // UTF-8 string
bool      // true, false
```

#### Composite Types

```ambient
// Records (structural by default)
{ x: number, y: number }

// Tuples
(number, string, bool)

// Enums (tagged unions, Rust-style)
enum Option<T> {
  Some(T),
  None,
}

enum Result<T, E> {
  Ok(T),
  Err(E),
}

// Collections (generic)
List<T>
Set<T>
Map<K, V>
```

#### Nominal Types

Use `unique` with a UUID to create nominally distinct types:

```ambient
unique(d098767b-4093-4d5c-ba37-ad92aa7b5d98) type UserId {
  value: string,
}

unique(a1b2c3d4-5678-90ab-cdef-1234567890ab) type OrderId {
  value: string,
}

// UserId and OrderId are incompatible even though structurally identical
```

#### Generics

```ambient
fn identity<T>(x: T): T {
  x
}

fn first<T, U>(pair: (T, U)): T {
  pair.0
}
```

#### Ability Types

Abilities in type position use `!` suffix:

```ambient
// A suspended ability that yields T and requires ability A
Ability<T, A!>

// Example: a suspended filesystem read
let op: Ability<string, Filesystem!> = Filesystem.read("file.txt");
```

### String Interpolation

Arbitrary expressions inside `${}`, must reduce to `string` type:

```ambient
let name = "world";
let greeting = "Hello, ${name}!";

let a = 1;
let b = 2;
let sum = "Sum: ${to_string(a + b)}";

let user = { name: "Alice", age: 30 };
let info = "User: ${user.name}";
```

### Lambdas

TypeScript-style syntax:

```ambient
// Single parameter
(a) => a + 1

// Multiple parameters
(a, b) => a + b

// No parameters
() => 42

// Multi-line
(a) => {
  let x = a + 1;
  x * 2
}

// Usage
List.map(nums, (a) => a + 1);
List.filter(users, (u) => u.age > 18);
```

---

## Abilities

Abilities are the mechanism for controlled side effects. They replace what other languages call "effects" but better capture that they represent what code _can do_.

### Declaring Abilities

```ambient
ability Filesystem {
  fn read(path: Path): string;
  fn write(path: Path, content: string): ();
  fn exists(path: Path): bool;
}

ability Network {
  fn fetch(request: Request): Response;
}

ability Console {
  fn print(message: string): ();
}
```

### Ability Dependencies

Abilities can depend on other abilities. The dependency is an implementation detail not exposed to callers:

```ambient
ability Log
  with Console
{
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}
```

When you use `Log`, you don't need to declare `Console`. The `Log` handler internally uses `Console`, but that's encapsulated. The `Console` handler used is relative to where `Log` is handled, not where `Log` is called.

### Abilities as Values

Abilities can be suspended and stored as values. Omit `!` to get the ability value instead of performing it:

```ambient
// Perform immediately
let content = Filesystem.read!("file.txt");

// Suspend as value
let read_op = Filesystem.read("file.txt");  // type: Ability<string, Filesystem!>

// Perform later
let content = read_op!;
```

This enables composition:

```ambient
let op1 = Filesystem.read("a.txt");
let op2 = Filesystem.read("b.txt");
let op3 = Network.fetch(request);

// Perform all concurrently
let [a, b, c] = Async.all!([op1, op2, op3]);

// Race: first to complete wins, others cancelled
let winner = Async.race!([op1, op2]);

// Variable-length list
let ops: List<Ability<string, Filesystem!>> = paths.map((p) => Filesystem.read(p));
let results = Async.all!(ops);
```

Ability values are handler-agnostic and can be serialized for remote execution.

### Ability Syntax in Type Signatures

Abilities appear after the return type in a `with` clause:

```ambient
// Single ability
fn read_config(path: Path): Config
  with Filesystem
{
  let content = Filesystem.read!(path);
  parse_config(content)
}

// Multiple abilities
fn fetch_and_log(url: Url): Response
  with Network, Log
{
  Log.info!("Fetching: ${url}");
  let response = Network.fetch!(Request { url: url, method: Get });
  Log.info!("Received: ${to_string(response.status)}");
  response
}

// No abilities (pure function)
fn add(x: number, y: number): number {
  x + y
}
```

### Ability Polymorphism

Functions can be polymorphic over abilities using ability variables (uppercase by convention):

```ambient
// E is an ability variable
fn map<T, U, E!>(list: List<T>, f: (T) -> U with E): List<U>
  with E
{
  // implementation
}

// Fixed abilities plus polymorphic
fn logged_map<T, U, E!>(list: List<T>, f: (T) -> U with E): List<U>
  with Log, E
{
  Log.info!("Mapping over list");
  map(list, f)
}
```

### Partial Ability Annotation

Use `_` for inferred abilities in public signatures:

```ambient
pub fn transform(x: Input): Output
  with Filesystem, _
{
  // Filesystem is required, other abilities inferred
}
```

### Handling Abilities

```ambient
fn main(): () {
  handle read_config("config.toml") {
    Filesystem.read(path) => {
      let content = host_read_file(path);
      resume(content)
    }
    Filesystem.write(path, content) => {
      host_write_file(path, content);
      resume(())
    }
  }
}
```

### Handlers as Values

Handlers can be defined as values and composed:

```ambient
let mock_fs: Handler<Filesystem> = {
  read(path) => resume("mock content"),
  write(path, content) => resume(()),
  exists(path) => resume(true),
};

let mock_network: Handler<Network> = {
  fetch(request) => resume(Response { status: 200, body: "{}" }),
};

// Use handler values
handle unit_test() with mock_fs, mock_network {}

// Override specific methods
handle unit_test() with mock_fs {
  Filesystem.read(path) => {
    Log.info!("Intercepted read: ${path}");
    resume("intercepted content")
  }
}
```

### Sandboxing

Restrict available abilities for untrusted code:

```ambient
sandbox with Log {
  // Only Log ability available here
  // Filesystem, Network, etc. are not accessible
  untrusted_code()
}

sandbox {
  // No abilities available - pure computation only
  pure_untrusted_code()
}
```

### Host Abilities

The host environment defines and provides abilities. Unhandled abilities at the top level are handled by the host.

Type checking requires knowing what abilities the host provides. If code uses an ability the host doesn't support, it's a type error.

For remote execution, the client must know what abilities the server offers.

---

## Concurrency

Concurrency is handled through the `Async` ability and ability values.

### Async Ability

```ambient
ability Async {
  // Wait for all to complete
  fn all<T, A!>(ops: List<Ability<T, A!>>): List<T>;

  // Wait for first to complete, cancel others
  fn race<T, A!>(ops: List<Ability<T, A!>>): T;
}
```

### Concurrent Execution

```ambient
fn fetch_both(url1: Url, url2: Url): (Response, Response)
  with Network, Async
{
  let op1 = Network.fetch(Request { url: url1, method: Get });
  let op2 = Network.fetch(Request { url: url2, method: Get });

  let results = Async.all!([op1, op2]);
  (results.0, results.1)
}

// Variable number of concurrent operations
fn fetch_all(urls: List<Url>): List<Response>
  with Network, Async
{
  let ops = List.map(urls, (url) => Network.fetch(Request { url: url, method: Get }));
  Async.all!(ops)
}

// Race between operations
fn fetch_fastest(urls: List<Url>): Response
  with Network, Async
{
  let ops = List.map(urls, (url) => Network.fetch(Request { url: url, method: Get }));
  Async.race!(ops)
}
```

### Cancellation

When `Async.race!` completes, losing operations are cancelled. Abilities should define cancellation behavior. This is handled at the handler level.

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

1. **Private functions**: Types and abilities fully inferred
2. **Public functions**: Return type and abilities must be declared; parameter types must be declared
3. **Local variables**: Always inferred
4. **Lambdas**: Parameter types inferred from context when possible

```ambient
// Private: everything inferred
fn helper(x, y) {
  x + y
}

// Public: must annotate
pub fn api(x: number, y: number): number {
  helper(x, y)
}

// Lambda: types from context
let doubled = List.map(numbers, (x) => x * 2);
```

---

## Operator Behavior

No operator overloading or value coercion. Types must match exactly:

- `if` requires `bool` condition
- Arithmetic operators (`+`, `-`, `*`, `/`, `%`) require `number` operands
- String concatenation uses interpolation: `"${a}${b}"` not `a + b`
- Comparison operators require matching types

A trait system may be introduced later.

---

## Semantics

### Immutability

All values are immutable. There is no mutation in the language.

Future consideration: a `Store<T>` ability for managed mutable references, or affine types. For now, model state through ability handlers or by threading state through function arguments.

### Content-Addressing

Every function is identified by a hash of:

1. Its fully-resolved type signature (including abilities)
2. Its implementation (normalized AST/bytecode)
3. Hashes of all functions it calls

#### Hash Stability

The hash is computed from a normalized form:

- Alpha-renaming: Local variable names don't affect hash
- Ordering: Best-effort normalization; false positives (different hash for same semantics) acceptable, false negatives (same hash for different semantics) not acceptable

#### Cycle Handling

Mutually recursive functions are hashed as a group:

1. Detect strongly-connected components in call graph
2. Assign a canonical ordering to SCC members
3. Hash the entire SCC as a unit

### Delimited Continuations

Abilities are implemented using single-shot delimited continuations.

**Single-shot constraint**: A continuation can be resumed at most once. Attempting to resume twice is a runtime error.

This supports:

- Exceptions (resume zero times)
- I/O abilities (resume once with result)
- State abilities (resume once with new state)
- Async abilities (resume once when operation completes)

Not supported:

- Backtracking / nondeterminism
- Multi-shot `amb` operators

### Execution Model

Single-threaded with cooperative concurrency:

1. User code runs on a single thread
2. Ability handlers may delegate to host thread pool
3. `Async.all!` and `Async.race!` suspend operations, let host execute them concurrently
4. Host resumes when operations complete
5. No shared mutable state between concurrent operations

---

## Remote Execution

### Function Execution

Send a function with arguments, receive a result:

```
Client                          Server
  |                               |
  |-- Execute(hash, args) ------->|
  |                               |
  |<-- NeedDeps([hash1, hash2]) --|  (if server missing dependencies)
  |                               |
  |-- Provide([fn1, fn2]) ------->|
  |                               |
  |<-- Result(value) -------------|  (or Error)
```

Running programs can live-upgrade themselves using a `Program.replace!(new_function)` ability replacing the current program.

For live reloading, the "server" is the local dev environment. For production, it's the deployed service.

### Ability Requirements

The remote must provide all ability handlers. No ability proxying back to caller.

If caller state is needed:

- Pass it as function arguments
- Or implement a streaming ability at the application level

### Wire Protocol

```
Message ::=
  | Execute { function: Hash, args: Value }
  | NeedDeps { hashes: List<Hash> }
  | Provide { functions: List<(Hash, CompiledFunction)> }
  | Result { value: Value }
  | Error { error: ErrorValue }

ErrorValue ::= {
  kind: ErrorKind,
  message: string,
  context: Option<Value>,
}

enum ErrorKind {
  MissingDependency,
  AbilityNotProvided,
  RuntimeException,
  TypeMismatch,
  Cancelled,
}
```

---

## Core Library

### Built-in Types

```ambient
type Path = string;
type Url = string;
type Duration = number;  // milliseconds
type Timestamp = number; // milliseconds since epoch

type Request = {
  url: Url,
  method: HttpMethod,
  headers: Map<string, string>,
  body: Option<string>,
};

type Response = {
  status: number,
  headers: Map<string, string>,
  body: string,
};

enum HttpMethod {
  Get,
  Post,
  Put,
  Delete,
  Patch,
}

type Range = {
  start: number,
  end: number,
};
```

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

ability Exception<E> {
  fn throw(error: E): !;
}

ability Console {
  fn print(message: string): ();
}

ability Log
  with Console
{
  fn debug(message: string): ();
  fn info(message: string): ();
  fn warn(message: string): ();
  fn error(message: string): ();
}

ability Async {
  fn all<T, A!>(ops: List<Ability<T, A!>>): List<T>;
  fn race<T, A!>(ops: List<Ability<T, A!>>): T;
}
```

### Standard Functions

```ambient
// Collections
fn List.map<T, U, E!>(list: List<T>, f: (T) -> U with E): List<U> with E;
fn List.filter<T, E!>(list: List<T>, f: (T) -> bool with E): List<T> with E;
fn List.fold<T, U, E!>(list: List<T>, init: U, f: (U, T) -> U with E): U with E;
fn List.concat<T>(a: List<T>, b: List<T>): List<T>;
fn List.length<T>(list: List<T>): number;
fn List.head<T>(list: List<T>): Option<T>;
fn List.tail<T>(list: List<T>): Option<List<T>>;

// Options
fn Option.map<T, U>(opt: Option<T>, f: (T) -> U): Option<U>;
fn Option.unwrap_or<T>(opt: Option<T>, default: T): T;
fn Option.and_then<T, U>(opt: Option<T>, f: (T) -> Option<U>): Option<U>;

// Results
fn Result.map<T, U, E>(res: Result<T, E>, f: (T) -> U): Result<U, E>;
fn Result.map_err<T, E, F>(res: Result<T, E>, f: (E) -> F): Result<T, F>;
fn Result.and_then<T, U, E>(res: Result<T, E>, f: (T) -> Result<U, E>): Result<U, E>;

// Strings
fn String.split(s: string, delimiter: string): List<string>;
fn String.join(parts: List<string>, delimiter: string): string;
fn String.trim(s: string): string;
fn String.contains(s: string, substring: string): bool;
fn String.length(s: string): number;

// Conversion
fn to_string<T>(value: T): string;
fn parse_number(s: string): Option<number>;
fn parse_bool(s: string): Option<bool>;
```

---

## Implementation

### Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Source Files (.ab)               │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│                      Parser                         │
│              (Source → CST → AST)                   │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│                  Type Checker                       │
│         (AST → Typed AST + Ability Info)            │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│                    Compiler                         │
│            (Typed AST → Bytecode)                   │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│              Content-Addressed Store                │
│           (Hash → Bytecode + Metadata)              │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│                 Bytecode VM                         │
│        (Execute with Ability Handlers)              │
└─────────────────────────────────────────────────────┘
```

### Bytecode VM

Stack-based virtual machine with:

- Value stack for operands
- Call stack for function frames
- Continuation stack for ability handlers

#### Instruction Set (Draft)

```
// Stack operations
PUSH_CONST <idx>      // Push constant from constant pool
PUSH_LOCAL <slot>     // Push local variable
POP                   // Discard top of stack
DUP                   // Duplicate top of stack

// Local variables
STORE_LOCAL <slot>    // Pop and store to local slot
LOAD_LOCAL <slot>     // Push local to stack

// Arithmetic (number operands only)
ADD, SUB, MUL, DIV, MOD, NEG

// Comparison (number operands only)
EQ, NE, LT, LE, GT, GE

// Logic (bool operands only)
AND, OR, NOT

// Control flow
JUMP <offset>         // Unconditional jump
JUMP_IF <offset>      // Jump if top of stack is true
JUMP_IF_NOT <offset>  // Jump if top of stack is false

// Functions
CALL <hash>           // Call function by hash
CALL_CLOSURE          // Call closure on stack
RETURN                // Return from function

// Abilities
SUSPEND <ability_idx> <method_idx>  // Create suspended ability value
PERFORM               // Perform ability value on stack
HANDLE <handler_offset>             // Install ability handler
RESUME                // Resume continuation with value
UNHANDLE              // Remove ability handler

// Data structures
MAKE_TUPLE <arity>    // Pop N values, push tuple
TUPLE_GET <idx>       // Get tuple element
MAKE_RECORD <field_count>  // Pop N values + field names, push record
RECORD_GET <field>    // Get record field
MAKE_CLOSURE <hash> <capture_count>  // Create closure
MAKE_ENUM <variant> <arity>  // Create enum variant

// Pattern matching
MATCH_TAG <variant> <jump_offset>  // Check enum variant, jump if no match

// Concurrency (host-assisted)
ASYNC_ALL <count>     // Pop N ability values, perform concurrently
ASYNC_RACE <count>    // Pop N ability values, race
```

### Content-Addressed Store

```rust
struct Store {
    // Hash -> compiled function
    functions: HashMap<Hash, CompiledFunction>,

    // Hash -> constant value
    constants: HashMap<Hash, Value>,

    // For debugging: Hash -> source location
    debug_info: Option<HashMap<Hash, DebugInfo>>,
}

struct CompiledFunction {
    hash: Hash,
    bytecode: Vec<u8>,
    constants: Vec<Value>,      // constant pool for this function
    dependencies: Vec<Hash>,    // functions this one calls
    type_signature: TypeSig,
    abilities: AbilitySet,
}

struct DebugInfo {
    source_file: String,
    source_map: Vec<(BytecodeOffset, SourceSpan)>,
    local_names: Vec<(Slot, String)>,
}
```

---

## CLI

The language is called `ambient`. File extension is `.ab`. Compiled files use `.ambient`.

```bash
# Compile source to bytecode
ambient compile foo.ab              # produces foo.ambient

# Run compiled program
ambient run foo.ambient

# Compile and run (convenience)
ambient run foo.ab

# Interactive REPL
ambient repl

# Start server for remote execution
ambient serve foo.ambient --port 8080

# Send function to remote server
ambient exec --server localhost:8080 --function my_fn --args '{"x": 42}'

# Hot reload (development)
ambient dev foo.ab                  # watches for changes, hot reloads
```

---

## Milestones

### Milestone 1: Bytecode VM Foundation

**Goal**: Execute hand-written bytecode with basic operations.

**Deliverables**:

- [x] Value representation (numbers, strings, bools, tuples, records)
- [x] Bytecode format and instruction set (core subset)
- [x] Stack-based VM execution loop
- [x] Arithmetic and comparison operations (no overloading)
- [x] Local variables
- [x] Function calls (direct, no closures yet)
- [x] Simple test harness for hand-written bytecode

**Test case**: Factorial, fibonacci, record manipulation.

### Milestone 2: Abilities and Handlers

**Goal**: Implement algebraic abilities with single-shot continuations.

**Deliverables**:

- [x] Continuation representation (captured stack segments)
- [x] SUSPEND instruction (create ability value)
- [x] PERFORM instruction (execute ability value)
- [x] HANDLE/UNHANDLE instructions
- [x] RESUME instruction
- [x] Single-shot enforcement (error on double resume)
- [x] Host-provided ability handlers (Rust callbacks)
- [x] Exception ability (throw + catch)
- [x] Console ability (print)

**Test case**: Exception handling, console I/O, state threading.

### Milestone 3: Abilities as Values

**Goal**: Suspend abilities and compose them.

**Deliverables**:

- [x] Ability value representation
- [x] Store ability values in variables
- [x] Pass ability values to functions
- [x] Serialize ability values

**Test case**: Create suspended ability, pass to another function, perform there.

### Milestone 4: Content-Addressed Store

**Goal**: Functions identified by hash, stored in content-addressed store.

**Deliverables**:

- [x] Hash computation for bytecode + type signature
- [x] Store implementation (in-memory HashMap)
- [x] Function lookup by hash
- [x] Dependency tracking
- [x] Cycle detection and SCC hashing
- [x] Serialization format for functions and values
- [x] Round-trip test: serialize → deserialize → execute

**Test case**: Store multiple functions, call between them by hash.

### Milestone 5: Remote Execution (Same Process)

**Goal**: "Send" a function to another VM instance for execution.

**Deliverables**:

- [x] VM instance isolation (separate stores, separate stacks)
- [x] Serialize function + dependencies
- [x] Transfer to second VM instance
- [x] Execute with host-provided ability handlers
- [x] Return result to caller

**Test case**: Two VM instances in unit test. Send function from A to B. B executes and returns result.

### Milestone 6: Remote Execution (Network)

**Goal**: Send a function to a remote machine over the network.

**Deliverables**:

- [x] Wire protocol implementation
- [x] Server: listen, receive function, execute, return result
- [x] Client: connect, send function, receive result
- [x] Dependency negotiation (server requests missing functions)
- [x] Error handling with proper error values

**Test case**: Server on localhost, client sends function, server executes and returns.

### Milestone 7: Type System (Core)

**Goal**: Hindley-Milner type inference for the core language.

**Deliverables**:

- [x] Type representation (primitives, functions, tuples, records, generics)
- [x] Type environment
- [x] Unification algorithm
- [x] Type inference (Algorithm W or J)
- [x] Structural type equivalence
- [x] Nominal types with `unique`
- [x] Type checking for hand-constructed ASTs
- [x] Type errors with context

**Test case**: Infer types for identity function, map function, record access.

### Milestone 8: Ability Types

**Goal**: Track abilities in the type system.

**Deliverables**:

- [x] Ability set representation
- [x] Ability variables (for polymorphism, with `!` syntax)
- [x] Ability row unification
- [x] Ability inference for function bodies
- [x] Ability checking at call sites
- [x] Ability dependency tracking (`with` clause on abilities)
- [x] `Ability<T, A!>` type for suspended abilities
- [x] Partial annotation with `_`

**Test case**: Infer abilities for function that reads a file. Verify ability polymorphism for `map`.

### Milestone 9: Concurrency

**Goal**: `Async` ability with concurrent execution.

**Deliverables**:

- [x] `Async.all!` implementation
- [x] `Async.race!` implementation with cancellation
- [x] Host integration for concurrent ability execution
- [x] Type checking for concurrent operations

**Test case**: Concurrent fetches (mocked), race with cancellation.

### Milestone 10: Parser (Lexer + CST)

**Goal**: Parse source files to concrete syntax tree.

**Deliverables**:

- [x] Lexer (tokens)
- [x] CST representation (preserves whitespace, comments)
- [x] Parser (recursive descent)
- [x] Error recovery (continue after errors)
- [x] Source spans on all nodes
- [x] String interpolation parsing

**Test case**: Parse example programs, round-trip CST to source.

**Implementation notes**: String interpolation lowering to AST is stubbed (runtime support needed).

### Milestone 11: Parser (AST + Lowering)

**Goal**: Lower CST to AST, resolve names.

**Deliverables**:

- [x] AST representation (desugared) - using ambient-engine::ast
- [x] Name resolution - qualified names to module definitions
- [x] Module system (`use` imports) - import resolution (single module)
- [x] Lowering from CST to AST
- [x] Source spans preserved for errors

**Test case**: Parse and lower example programs, feed to type checker.

**Implementation notes**: Name resolution is implemented with support for:

- Module-level definitions (functions, constants, types, enums, abilities)
- Local scopes (parameters, let bindings, pattern bindings)
- Import resolution within single modules
- Cross-module imports will be added with the module registry in Milestone 12

### Milestone 12: Compiler Pipeline

**Goal**: Full pipeline from source to execution.

**Deliverables**:

- [x] Source → CST → AST → Bytecode
- [x] CLI: `ambient compile`, `ambient run`, `ambient check`, `ambient ast`
- [x] Integration tests for end-to-end compilation
- [x] Typed AST (module-level type checking)
- [x] Content-addressed store integration (hash computation from bytecode content)

**Test case**: Write programs in `.ab` files, compile and run.

**Implementation notes**: The compiler pipeline (`ambient-engine/src/compiler.rs`) compiles AST to bytecode with support for:

- Function definitions with parameters and return values
- Recursive and mutually recursive functions (stable hash computation)
- Literals (numbers, booleans, strings)
- Binary and unary operators
- If-else expressions
- Let bindings
- Function calls (both local and external)

The CLI supports:

- `compile`: Source → .ambient bytecode file (JSON serialized)
- `run`: Execute .ab source or .ambient bytecode
- `check`: Validate syntax and type-check the module
- `ast`: Dump the AST for debugging

Type checking integration (`ambient-engine/src/infer.rs`):

- `check_module()`: Module-level type inference with Hindley-Milner algorithm
- Phase 1: Collect all function signatures into type environment
- Phase 2: Type-check each function body against its signature
- Ability tracking and verification against declared abilities
- Rich error messages with source context and suggestions

### Milestone 13: Handlers as Values

**Goal**: First-class handler values for testing and composition.

**Deliverables**:

- [x] Handler literal syntax (parser: `{ method(params) => body, ... }`)
- [x] Handler type (`Handler<A>`) in type system
- [x] `HandlerValue` runtime representation
- [x] CST and AST lowering for handler literals
- [x] Type checking for handler literals (matching ability signatures)
- [x] Bytecode compilation for handler literals
- [x] `handle ... with handler_value` syntax (parsing and type checking)
- [x] Handler composition runtime support (`HandlerValue::compose`)
- [x] `handle ... with handler_value` runtime installation (`HandleWithValue` opcode)
- [x] Handler composition via `handle ... with handler_value { inline_overrides }` syntax

**Test case**: Define mock handlers, use in tests.

**Implementation notes**: The parser recognizes handler literals as `{ method(params) => body, ... }` syntax. The type system includes `Handler<AbilityId>` and the runtime has `HandlerValue` with method-to-function mappings. Type checking infers the ability from method names and validates signatures. Bytecode compilation uses `MakeHandler` opcode to create handler values at runtime.

The `handle expr with handler1, handler2 { ... }` syntax is fully supported. Handler values in the `with` clause are type-checked to ensure they are `Handler<A>` types. The `HandleWithValue` opcode (0xB1) dynamically installs handlers from `HandlerValue` objects at runtime. The `HandlerFrame` struct supports both inline handlers (`HandlerKind::Inline`) and value-based handlers (`HandlerKind::Value`), and the `Perform` opcode correctly dispatches to the appropriate method function.

Handler composition is achieved through the `handle ... with handler_value { inline_overrides }` syntax, where a complete handler value provides the base implementation and inline handlers override specific methods. This avoids the need for partial handlers (which would be invalid) while still enabling layered behavior.

### Milestone 14: Sandboxing

**Goal**: Restrict abilities for untrusted code.

**Deliverables**:

- [x] `sandbox with abilities { ... }` syntax
- [x] Ability restriction enforcement
- [x] Type checking for sandbox blocks

**Test case**: Run untrusted code with limited abilities, verify restrictions.

**Implementation notes**: Sandboxing is implemented as a compile-time feature that restricts which abilities can be used within a block. The implementation includes:

- **Parser** (`ambient-parser/src/parser.rs`): `parse_sandbox_expr()` parses `sandbox with Ability1, Ability2 { body }` or `sandbox { body }` (pure computation).

- **CST/AST** (`cst.rs`, `ast.rs`): `CstSandboxExpr` and `SandboxExpr` with `allowed_abilities` (list of ability names) and `body` expression.

- **Type Checking** (`infer.rs`): The type checker:
  1. Saves the current ability context
  2. Resets to empty abilities
  3. Type-checks the body expression
  4. Verifies the body only uses abilities in the `allowed_abilities` list
  5. Reports `SandboxAbilityViolation` error if unauthorized abilities are used
  6. Restores the original ability context

- **Bytecode Compilation** (`compiler.rs`): Since sandbox is purely compile-time, the body is compiled directly with no special bytecode. The type system has already verified ability restrictions.

- **Error Messages**: `TypeErrorKind::SandboxAbilityViolation` reports which ability was used and what abilities were allowed.

Example usage:
```ambient
// Pure sandbox - no abilities allowed
sandbox {
    pure_computation()
}

// Restricted sandbox - only Log ability
sandbox with Log {
    plugin_code()  // Can only use Log, not Filesystem, Network, etc.
}
```

### Milestone 15: Standard Library

**Goal**: Useful core library.

**Deliverables**:

- [x] List value type and operations (`MakeList`, `ListGet`, `ListLength`, `ListConcat`, `ListAppend`, `ListHead`, `ListTail`)
- [x] String functions (`StringLength`, `StringSplit`, `StringJoin`, `StringTrim`, `StringContains`, `StringConcat`)
- [x] Type conversion (`ToString`, `ParseNumber`, `ParseBool`)
- [x] Time ability implementation (`now`, `wait`)
- [x] Random ability implementation (`seed`, `in_range`)
- [x] Log ability (debug, info, warn, error with level filtering)
- [x] `register_all_standard_abilities()` convenience function
- [x] Map collection type and operations (`MakeEmptyMap`, `MapGet`, `MapInsert`, `MapRemove`, `MapContains`, `MapLength`, `MapKeys`, `MapValues`)
- [x] Set collection type and operations (`MakeEmptySet`, `MakeSet`, `SetInsert`, `SetRemove`, `SetContains`, `SetLength`, `SetUnion`, `SetIntersection`, `SetDifference`, `SetToList`)
- [ ] Option/Result utilities (future)

**Test case**: Write useful programs using standard library.

**Implementation notes**: The standard library is implemented as bytecode opcodes for performance-critical operations (lists, strings, type conversion) and as host-provided ability handlers for I/O operations (Time, Random, Log). The `register_all_standard_abilities()` function registers Console, Exception (fallback), Time, Random, and Log abilities on a VM.

### Milestone 16: Developer Experience

**Goal**: Tooling for productive development.

**Deliverables**:

- [x] REPL with ability handlers
- [x] Error messages with source context
- [ ] Debug info generation (source maps)
- [ ] `ambient dev` with hot reload
- [x] Basic LSP server (hover, go-to-definition, diagnostics, completions)

**Test case**: Interactive development workflow.

**Implementation notes**: The REPL (`ambient repl`) provides an interactive evaluation environment with:

- Line editing support via rustyline
- Expression evaluation with immediate results
- All standard abilities registered (Console, Time, Random, Log, Exception)
- REPL commands (`:help`, `:quit`, `:clear`)
- Formatted error display with source context and caret underlining

Error messages now include source context for both parse and type errors:
- Line numbers and column positions
- Source line display with error underlining
- Context notes where applicable
- Consistent formatting across CLI commands and REPL

The LSP server (`ambient-lsp` crate) provides IDE support:
- Diagnostics: Parse errors and type errors published on document changes
- Hover: Type information for expressions
- Go-to-definition: Navigation to function and variable definitions
- Completions: Auto-completion for keywords, types, abilities, functions, and local variables
- Full document sync for real-time analysis

---

## Future Work (Out of Scope)

- WASM target
- Embedded target (FFI/crate)
- Bidirectional filesystem sync
- Package manager
- Type unions (`A | B`)
- Multi-shot continuations
- Actor model
- Trait system / operator overloading
- Mutable references (`Store<T>` ability or affine types)
- Incremental compilation (only recompile changed functions)

---

## Appendix: Example Programs

### Hello World

```ambient
pub fn main(): ()
  with Console
{
  Console.print!("Hello, world!");
}
```

### Factorial

```ambient
fn factorial(n: number): number {
  if n <= 1 {
    1
  } else {
    n * factorial(n - 1)
  }
}
```

### File Processing

```ambient
pub fn word_count(path: Path): number
  with Filesystem
{
  let content = Filesystem.read!(path);
  let words = String.split(content, " ");
  List.length(words)
}
```

### HTTP Request with Error Handling

```ambient
pub fn fetch_json(url: Url): Result<Json, string>
  with Network
{
  let response = Network.fetch!(Request {
    url: url,
    method: Get,
    headers: Map.empty(),
    body: None,
  });

  if response.status == 200 {
    match parse_json(response.body) {
      Some(json) => Ok(json),
      None => Err("Invalid JSON"),
    }
  } else {
    Err("HTTP error: ${to_string(response.status)}")
  }
}
```

### Concurrent Fetching

```ambient
use HttpMethod.*;

pub fn fetch_all_users(ids: List<UserId>): List<User>
  with Network, Async
{
  let ops = List.map(ids, (id) => {
    Network.fetch(Request {
      url: "https://api.example.com/users/${id.value}",
      method: Get,
      headers: Map.empty(),
      body: None,
    })
  });

  let responses = Async.all!(ops);
  List.map(responses, (r) => parse_user(r.body))
}
```

### Racing Requests

```ambient
pub fn fetch_fastest(urls: List<Url>): Response
  with Network, Async
{
  let ops = List.map(urls, (url) => {
    Network.fetch(Request { url: url, method: Get, headers: Map.empty(), body: None })
  });
  Async.race!(ops)
}
```

### Ability Handling

```ambient
fn run_with_logging<T, E!>(f: () -> T with Log, E): T
  with Console, E
{
  handle f() {
    Log.info(message) => {
      Console.print!("[INFO] ${message}");
      resume(())
    }
    Log.warn(message) => {
      Console.print!("[WARN] ${message}");
      resume(())
    }
    Log.error(message) => {
      Console.print!("[ERROR] ${message}");
      resume(())
    }
    Log.debug(message) => {
      Console.print!("[DEBUG] ${message}");
      resume(())
    }
  }
}
```

### Testing with Mocks

```ambient
let mock_fs: Handler<Filesystem> = {
  read(path) => resume("mock file content"),
  write(path, content) => resume(()),
  exists(path) => resume(true),
};

let mock_network: Handler<Network> = {
  fetch(request) => resume(Response {
    status: 200,
    headers: Map.empty(),
    body: "{}",
  }),
};

fn test_my_function(): () {
  handle my_function() with mock_fs, mock_network {}
}
```

### Sandboxed Execution

```ambient
fn run_plugin(plugin: () -> string with Log): string
  with Log
{
  // Plugin can only use Log, nothing else
  sandbox with Log {
    plugin()
  }
}
```

### State via Ability Handler

```ambient
ability Counter {
  fn increment(): ();
  fn get(): number;
}

fn with_counter<T, E!>(initial: number, f: () -> T with Counter, E): (T, number)
  with E
{
  let ref = Ref.new!(0);
  let result = handle f() {
    Counter.increment() => {
      Ref.set!(ref, Ref.value!(ref) + 1);
      resume(())
    }
    Counter.get() => {
      resume(Ref.value!(count))
    }
  };
  (result, count)
}
```
