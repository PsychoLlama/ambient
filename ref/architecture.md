# Ambient Language Reference

A content-addressed, ability-based programming language inspired by Unison, Rust, and TypeScript.

This document is the top-level authority on the language design. Deeper
treatments of individual subsystems live in sibling documents, linked
from each section:

- [modules.md](modules.md) — modules, `Fqn` identity, `use`, the `core::` hierarchy
- [types.md](types.md) — primitives, records, tuples, generics, nominal structs and enums
- [traits.md](traits.md) — traits, impls, operators, inherent impls, dispatch/coherence
- [abilities.md](abilities.md) — abilities, handlers, sandboxing, `core::system`, error handling
- [core-library.md](core-library.md) — core abilities and the standard function set
- [processes.md](processes.md) — the (experimental) process model and live upgrade
- [remote-execution.md](remote-execution.md) — running code by hash on a remote

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
// value is content-addressed into a standalone object when the module is
// built and referenced by hash (deduplicated, never re-inlined); two consts
// with the same value share one object. This is deliberately minimal and may
// widen later. The type annotation is optional — it is inferred from the
// literal when omitted (`const PI = 3.14159;`). A `const` is valid at module
// scope and inside any block, wherever a `use` is.
const PI: Number = 3.14159;

// Functions
fn add(x: Number, y: Number): Number {
  x + y
}

// Public functions (exported)
pub fn multiply(x: Number, y: Number): Number {
  x * y
}
```

### Modules

Modules map 1:1 to files under `src/`; a directory is a namespace module,
and **every item in a build has exactly one fully-qualified identity** — a
first-class `Fqn`, not a string. Path roots are `pkg` (package root), `self`
(same directory), `super` (parent, chainable), and `core` (standard library,
with host bindings under `core::system`). `use` takes a Rust-style use-tree,
and anything reachable by its fully-qualified path works through `use` and
vice versa.

See **[modules.md](modules.md)** for the full treatment of `Fqn` identity,
the `use` grammar, re-exports, and how the `core::` hierarchy is built.

**Note:** the `Fqn` is a _location_ axis — a compile-time
lookup key and post-hash label — never a _content_ axis. It must never
feed a content hash: a hash pins an item's implementation and its
transitive dependencies, never the name it resolves to, so renaming or
re-scoping an item leaves every hash untouched.

### Types

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

// Enums are nominal: every declaration carries a mandatory `unique(<uuid>)`
// prefix, so two structurally identical enums are distinct types.
unique(E1B2C3D4-0000-0000-0000-000000000001) enum Shape { Circle(Number), Square(Number), Dot }

// Nominal structs (structurally identical but incompatible)
unique(D098767B-4093-4D5C-BA37-AD92AA7B5D98) struct UserId { value: String }

// Generics
fn identity<T>(x: T): T { x }
```

See **[types.md](types.md)** for records, tuples, generics, and the full
story on nominal identity — why `unique(<uuid>)` is mandatory on structs
and enums, and how `Option`/`Result` carry reserved UUIDs.

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

## Traits

Traits define shared behavior for nominal types; types also take methods
directly through inherent impls. Standard operators (`+`, `==`, `<`, ...)
dispatch to prelude traits (`Add`, `Eq`, `Ord`, ...). Dispatch is fully
static and content-addressed — impl methods compile to ordinary named
functions under a canonical symbol, so there is no runtime trait registry.

See **[traits.md](traits.md)** for defining and implementing traits,
operator overloading, associated functions, inherent impls, and how
dispatch, coherence, and content-addressing survive live upgrade.

## Abilities

Abilities are the mechanism for controlled side effects. They are
**nominal**, like enums and structs: a mandatory `unique(<uuid>)` prefix is
the ability's identity, so renaming or moving a declaration never changes
it and two same-shaped abilities never collide. Each method carries a
mandatory **default implementation** — the body an unhandled perform runs —
and the method's identity is a content hash of the ability uuid, the
canonical signature, and that implementation (never the method's name).
Functions declare the abilities they perform with a `with` clause;
`with ... handle` installs handlers using single-shot delimited
continuations. The platform abilities (Stdio, FileSystem, Network, ...) are
ordinary in-language declarations under `core::system` whose default
implementations call the platform's `extern fn`s.

See **[abilities.md](abilities.md)** for ability identity, declaring and
performing abilities, default implementations, ability polymorphism,
handlers as values, sandboxing, the `core::system` host-binding split, and
error handling.

## Concurrency

All IO is blocking. There is no `Async` ability and no async/await-style
primitives — this is intentional. A perform like `core::system::Network::receive!`
simply blocks the calling code until the native call returns.

Concurrency comes from the Erlang-inspired **process model** (see
[processes.md](processes.md)): named reducer processes with isolated state,
communicating by message passing through the `core::system::Process`
ability. Each process runs on its own thread with its own VM, so a
blocked process blocks only itself.

This design is motivated by live-upgrade correctness — hot code
replacement needs a well-defined unit of state to hand off, and a
process mailbox/reducer boundary provides exactly that. It is, however,
**experimental**: prototyping has shown it is not a perfect fit for live
upgrades, and the design may yet pivot to a different unit of state.
See [processes.md](processes.md) for the current model and its caveats,
and Future Work below for where this is headed.

## Error Handling

Errors are abilities: `Exception::throw!` raises, and the nearest enclosing
`with ... handle` for Exception catches (catch-and-continue). Fallible host
operations currently raise a catchable `Exception` at the call site rather
than returning `Result` (the resume-with-substitute part of that pattern is
slated for removal in favor of plain `Result` returns on fallible APIs).
`Option`/`Result` remain ordinary data types for domain modeling.

See **[Error Handling in abilities.md](abilities.md#error-handling)** for the
full treatment, including host failures as exceptions and the
Option/Result-vs-exceptions distinction.

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

## Core Library

The core library ships as ordinary content-addressed Ambient modules. Core
abilities (Time, Random, Stdio, Log, FileSystem, Process, Env, ...) are
declared in the platform bindings interface; Option, Result, and List expose
their combinators and predicates as inherent methods so pipelines read
receiver-first.

See **[core-library.md](core-library.md)** for the ability declarations and
the full set of standard functions by type.

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
parse → check → diagnose pipeline, and it is the _only_ place that
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
callees, and each perform site's ability-method reference — the ability
uuid, canonical signature, and default-implementation hash), arity/locals
metadata, and the dependency list — so the hash pins the implementation,
every transitive dependency, _and_ the exact identity and behavior of
every ability method the function performs. One encoding serves as
the unit of hashing, storage, and network transfer, which makes every object
self-verifying: re-hash the bytes and compare.

Three kinds of code objects (plus leaf `Value` objects for `const`s):

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
- **Native** — an `extern fn`'s identity: exactly `(uuid, param_count)`,
  nothing else. The host binding supplies the uuid, so renaming the
  declaration never moves the hash, and a receiving VM binds the uuid
  against its own native table or fails loudly. See
  [core-library.md](core-library.md#native-functions-extern-fn).

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
   and [processes.md](processes.md))

## Remote Execution

Remote execution servers are written in Ambient itself on the `Network`
(TCP) and `Execute` (run-by-hash) abilities. Code ships as canonical object
packs — receivers recompute every hash from the bytes — and runs in an
isolated VM whose effects come from host grants or shipped handler values.

See **[remote-execution.md](remote-execution.md)** for the protocol, the
grant/shipped-handler split, and value serialization.

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

## Example Programs

### Hello World

```ambient
pub fn run(): () with core::system::Stdio {
  core::system::Stdio::out!("Hello, world!");
}
```

### Factorial

```ambient
fn factorial(n: Number): Number {
  if n <= 1 { 1 } else { n * factorial(n - 1) }
}
```

### Vector Math with Traits

```ambient
// Add and Eq come from the prelude; implementing them enables + and ==.
unique(A1B2C3D4-0000-0000-0000-000000000001) struct Vec2 { x: Number, y: Number }

impl Add for Vec2 {
  fn add(self, other: Vec2): Vec2 {
    Vec2 { x: self.x + other.x, y: self.y + other.y }
  }
}

impl Eq for Vec2 {
  fn eq(self, other: Vec2): Bool {
    self.x == other.x && self.y == other.y
  }
}

fn run(): Bool {
  let a = Vec2 { x: 1, y: 2 };
  let b = Vec2 { x: 3, y: 4 };
  let c = a + b;              // Vec2 { x: 4, y: 6 }
  c == Vec2 { x: 4, y: 6 }    // true
}
```

### Testing with Mocks

```ambient
let mock_fs: Handler<FileSystem> = {
  FileSystem::read(path) => resume("mock content"),
  FileSystem::write(path, content) => resume(()),
  FileSystem::exists(path) => resume(true),
};

fn test_my_function(): () {
  with mock_fs handle my_function()
}
```

## Future Work

Roughly in priority order:

- **Grow the standard library as ordinary modules.** The module system
  treats `core` as a real package (compiled `.ab` modules, whole-module and
  item imports, qualified calls), and every native operation is now a
  declared `extern fn` — but the library itself is small (list/math/string
  helpers). Target roughly the granularity of Go's or Node's standard
  libraries. Generic trait bounds would unlock `contains`/`sort_by`.
- **Cross-module ability imports.** The platform-bindings split is
  done: platform abilities are in-language declarations (`platform.ab`)
  whose default implementations call module-private extern fns, the
  engine crate knows only Exception, and embedders bind natives by
  uuid. What remains is the general form: exporting an `ability` from
  one user module and importing it in another (exports carry the kind
  but consumers don't hydrate them yet), plus REPL registration of
  user-declared abilities.
- **Workspace mechanism (multi-package local development).** Resolve
  sibling packages by name, share a build directory, and compile
  independent packages in parallel. Lands before the package manager, and
  the `Fqn` scope machinery (`Workspace(pkg)`) is already shaped for it.
- **Formalizing live upgrades.** Live, in-place code replacement is a
  core goal, but the mechanism is still open research. The current
  experiment is the Erlang-style process model ([processes.md](processes.md)):
  a reducer/mailbox boundary as the unit of state to hand off, with
  `ambient dev` re-running the entry function as a reconciliation pass.
  Prototyping has shown this is **not a perfect fit** — the state-handoff
  contract is coarse (a faulted reduction restarts from a fresh init) and
  a process boundary may be the wrong unit — so we may pivot to a
  different design. What needs formalizing regardless of the vehicle: the
  deploy-pass/reconciliation model, an explicit state-migration contract
  (today's default is "the new reducer consumes the old state or the
  process restarts"), and an `Execute`-driven remote deploy path (live
  upgrade over the network with the mechanics tested locally). Process
  features that only matter if the process model wins — typed `spawn` via
  effect-polymorphic ability signatures, linking/monitors, supervision
  trees, receive timeouts — are contingent on that decision.
- Generic traits, supertraits, trait bounds (`fn foo<T: Eq>(x: T)`) — only
  if needed; traits exist to support polymorphic operators
- Incremental compilation backed by the persisted store
- WASM target
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
