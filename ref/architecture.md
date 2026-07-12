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
- [live-upgrade.md](live-upgrade.md) — live upgrade: generations, late-bound names, cells, tasks, drain
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
The carve-out is never-returning methods (`: !`), which may stay abstract:
performing one unwinds to the handler (no continuation is captured), so an
unhandled abstract perform is a runtime fault.
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

Concurrency comes from **tasks** (see [live-upgrade.md](live-upgrade.md),
"Tasks"): named, supervised, drainable loops declared through the
`core::system::Task` ability. Each task runs on its own thread with its
own VM, so a blocked task blocks only itself; the runtime-owned `State`
cell table and network handle table are the only cross-task state.
There are no mailboxes and no message passing — tasks coordinate
through cells and sockets.

This design is motivated by live-upgrade correctness: the runtime owns
all long-lived state (cells) and re-resolves each task body's deployed
name before every pass, so a deploy lands at well-defined boundaries
with nothing to hand off. An earlier Erlang-style mailbox/reducer
process model was prototyped for the same goal and retired — see
[live-upgrade.md](live-upgrade.md), "Relation to the process model".

## Error Handling

Errors are abilities: `Exception::throw!` raises, and the nearest enclosing
`with ... handle` for Exception catches (catch-and-continue). Exception is
**catch-only** — `throw` returns `!` (never), so the perform site unwinds
and a handler arm cannot `resume` a failing operation with a substitute
value. A `!`-typed expression fits any context (bottom elimination:
`if c { n } else { Exception::throw!("...") }` is a `Number`), and any
user-declared ability method may return `!` for the same catch-only
semantics. Fallible host operations return
`Result<T, String>` (`FileSystem::read`, every `Network` method, ...) and
are matched on like ordinary data; only unwired capabilities and runtime
control errors still travel the catchable `Exception` channel, as hard
failures. `Option`/`Result` remain ordinary data types for domain modeling.

See **[Error Handling in abilities.md](abilities.md#error-handling)** for the
full treatment, including fallible host operations returning `Result` and the
Option/Result-vs-exceptions distinction.

## Type Inference Rules

1. **Public functions**: The full signature must be declared — every
   parameter type and the return type. A `pub` signature is the contract
   other modules compile against (importers rebuild the callable type from
   the written annotations alone), so inference can never fill an omitted
   position for a foreign caller. Abilities must be declared (`with ...`);
   no clause means pure. Using an undeclared ability is a type error.
2. **Private functions**: Abilities are inferred from the body when no
   `with` clause is given (a clause, if present, is enforced). Inferred
   abilities propagate to callers and count against the declarations of any
   public function that transitively reaches them. Unannotated parameter
   and return types are likewise inferred — each such position is a single
   **monomorphic** type variable shared by the function's body and every
   call site, so the body's constraints reach callers and vice versa
   (`fn g(x) { x + 1 }` pins `x: Number`; `g(true)` is a type error). One
   consequence: an unannotated position of a _generic_ function may not
   resolve to one of its own type parameters (a monomorphic variable cannot
   carry a quantified type) — the checker asks for the annotation.
3. **Local variables**: Always inferred
4. **Lambdas**: Parameter types inferred from context; the lambda's ability
   set is inferred from its body and carried on its function type
5. **Effect propagation**: Calling a function requires the caller to provide
   (declare, infer, or handle) the callee's abilities

## Core Library

The core library ships as ordinary content-addressed Ambient modules. Core
abilities (Time, Random, Stdio, Log, FileSystem, Task, Env, ...) are
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

**Proper tail calls.** Every call in tail position — the trailing
expression of a function, lambda, or handler arm, threaded through `if`
branches, `match` arms, block results, and `sandbox` — reuses the
current frame (`TailCall`/`TailCallClosure`) instead of pushing a new
one, so tail recursion runs in constant stack space. Non-tail
recursion still caps at `max_call_depth` (1000). A tail-position
`resume` is constant-space too: it compiles to `TailResume`, which
discards the arm frame before reinstating the continuation (a fused
`Resume; Return`), so a handler that resumes every cycle no longer
parks a frame per cycle. One carve-out remains _not_ a tail position:
a `handle` expression's body (trailing continuation cleanup runs after
it).

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
  `<type-uuid>::<trait-uuid>::<method>`, inherent methods
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
2. Concurrency is tasks: each task owns a thread and a VM, sharing only
   the runtime-owned cell and handle tables (see Concurrency and
   [live-upgrade.md](live-upgrade.md))

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
                           #   deploys each change onto the running system
                           #   (cells keep state, tasks pick up rebound
                           #   names; plain rerun for task-less programs)
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
  libraries. Generic trait bounds now exist (`fn f<T: Eq>` — see
  [traits.md](traits.md#generic-constraints)) and already power
  `List::contains`/`index_of`/`min`/`max`/`sorted`; the remaining blockers
  are the bound-dispatch-inside-lambdas and conditional-impl gaps noted
  there.
- **Cross-module ability imports (done).** The platform-bindings split
  is done: platform abilities are in-language declarations
  (`platform.ab`) whose default implementations call module-private
  extern fns, the engine crate knows only Exception, and embedders bind
  natives by uuid. The general form is done too: exporting an `ability`
  from one user module and importing it in another works end-to-end
  through checker, compiler, and VM — bare import, fully-qualified use,
  default implementations, dependency (`with`) rows, re-exports, and
  nominal-typed method signatures all cross the module boundary. The
  REPL registers `core::system` and every project module, and honours an
  `ability` declared in one turn and used in a later one
  (`crates/ambient-cli/tests/module_system.rs`, `repl_tests.rs`).
- **Workspace mechanism (multi-package local development).** Resolve
  sibling packages by name, share a build directory, and compile
  independent packages in parallel. Lands before the package manager, and
  the `Fqn` scope machinery (`Workspace(pkg)`) is already shaped for it.
- **Live upgrades are implemented**:
  **[live-upgrade.md](live-upgrade.md)** — upgrades as name rebindings
  applied through deployable generations, with author-placed late-bound
  points (the `Live` ability), runtime-owned state cells with an
  explicit migration contract, drainable named tasks, exact generation
  retirement, and three deploy frontends (the dev loop, the REPL, and
  remote deploy over the `Deploy` ability). The in-language loop idiom
  is now spellable — proper tail calls mean a task's loop can re-enter
  through `Live::latest!` in tail position and run in constant stack
  space (see `examples/deploy_server`'s `converse`), and a
  tail-position `resume` runs in constant space too (`TailResume`), so
  a handler-driven effect loop is no longer frame-bounded. Task bodies
  and `State` functions are now checker-enforced: ability signatures
  carry effect-polymorphic function parameters
  (`Task::ensure<E!>(name, body: () -> () with E)`), so a malformed body
  or state function is a compile error. Still open: typed `Live::latest`
  (blocked on arity polymorphism — it is applied at every arity, so no
  single function type is precise) and snapshot groups for multi-read
  consistency.
- Trait bounds (`fn foo<T: Eq>(x: T)`) are **done** — dictionary-passing,
  uniform across functions, impl methods, and ability methods. Still open:
  generic traits (`trait Container<T>`), supertraits, conditional impls as
  dictionary sources, bound dispatch inside lambdas, and bounded generics
  as first-class values.
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
