# Abilities

Part of the [Ambient Language Reference](architecture.md).

Abilities are the mechanism for controlled side effects.

## Ability Identity

Abilities are **nominal**, exactly like enums and structs: the mandatory
`unique(<uuid>)` prefix _is_ the ability's identity
(`AbilityId::from_uuid`). Renaming an ability, renaming its methods, or
moving the declaration to another module never changes it, and two
same-shaped abilities with different uuids never unify — the same rules
`unique(...)` gives every other nominal type.

A **method's** identity is a `MethodKey`: the blake3 hash of the ability
uuid, the method's canonical signature (type variables numbered by first
occurrence, so `<T>(T) -> U` encodes identically everywhere), and the
content hash of its **default implementation**. Two things are deliberate
here:

- The method _name_ is excluded — renaming a method never moves its key.
- The default implementation is included — two same-signature methods in
  one ability stay distinct as long as their bodies differ (`Stdio::out`
  calls `extern fn stdio_out`, `Stdio::err` calls `stdio_err`), and
  changing a method's behavior re-keys it loudly instead of silently
  binding old callers to new semantics.

Dispatch keys on `(AbilityId, MethodKey)`: compiled bytecode references
each performed method through the constant pool (uuid, signature, and
implementation hash), so a function's content hash commits to the exact
identity _and behavior_ of every ability method it performs, and a
handler matches a perform only if both sides derived the same key. That
is what makes remote execution sound: a function compiled against
version N of an ability cannot silently dispatch against a handler
compiled for version N+1.

## Declaring Abilities

Modules declare abilities with `unique(<uuid>) ability`. Every method
carries a **default implementation**: the body that runs — as an ordinary
function call at the perform site — when no handler is in scope. The
type checker verifies each body against the declared signature, and its
allowed effects are exactly the ability's declared `with`-dependencies
(none means the body is pure). A body can therefore never perform its own
ability, which is also what keeps method identity well-founded: an
implementation hash never depends on itself.

```ambient
// Default implementations bottom out in extern fns (the pure host
// boundary) or plain values.
extern fn read_file(path: String): String;

unique(A1B2C3D4-0000-0000-0000-000000000001) ability FileSystem {
  fn read(path: String): String {
    read_file(path)
  }
  fn exists(path: String): Bool {
    false                        // conservative default
  }
}

unique(A1B2C3D4-0000-0000-0000-000000000002) ability Picker {
  fn pick<T>(a: T, b: T): T {    // generic methods instantiate per call
    a
  }
}

// Abilities can depend on other abilities: the dependency row is what
// the default bodies may perform, and performing Log also requires
// Stdio in the caller's effect row.
unique(A1B2C3D4-0000-0000-0000-000000000003) ability Log with core::system::Stdio {
  fn info(message: String): () {
    core::system::Stdio::out!("info: ${message}")
  }
}
```

Because the default implementation is an ordinary content-addressed
function, it ships in packs like any code: a perform site depends on its
methods' implementations, so remote code always carries its fallback
behavior with it.

The platform abilities (Stdio, FileSystem, Network, ...) are themselves plain
`ability` declarations — see "The `core::system` module" below. Handlers
override defaults: `with ... handle` expressions and handler values
intercept a perform before the default implementation runs. Abilities
import across modules like any other item: `use
pkg::b::SomeAbility;` (and `use core::system::Network;`) brings the ability into
scope under its bare name, and every ability is also reachable fully
qualified with no import (`with pkg::b::SomeAbility`,
`pkg::b::SomeAbility::method!(…)`) — the same rule as every other item.
Current limit: the REPL does not yet register `core::system` as a module, so
bare `use core::system::…` there is a follow-up.

## Using Abilities

```ambient
// Perform with ! (FileSystem here is the module-local declaration above;
// the platform ability would be core::system::FileSystem::read!)
let content = FileSystem::read!("file.txt");
```

Every module's abilities are in scope _fully-qualified_ (`core::system::Stdio`
or `pkg::effects::Counter` in performs, `with` clauses, effect-row
annotations, handler arms, and sandbox clauses) with no `use` — the same
rule as every other item. To drop the prefix, import the ability:
`use core::system::Stdio;` then `with Stdio` and `Stdio::out!(...)` work
bare thereafter. A bare `Stdio` that was _never_ imported (and is not a
local declaration) is a type error — the diagnostic suggests qualifying with
`core::system::` or adding the `use`. A local `ability Stdio` shadows an
imported one under the bare name; the platform one stays reachable
qualified. The builtin `Exception` is always bare and may not be spelled
with a namespace.

## Ability Syntax in Type Signatures

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
  with core::system::Network, Log
{ ... }

// No abilities (pure function)
fn add(x: Number, y: Number): Number { x + y }
```

## Ability Polymorphism

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

## Handling Abilities

```ambient
fn run(): () {
  with {
    FileSystem::read(path) => {
      let content = "contents from anywhere";
      resume(content)
    }
  } handle read_config("config.toml")
}
```

A handle expression reads as English — `with H1, ..., Hn handle BODY
[else E]` — "with these handlers, handle this body". The `with` list is
one or more handlers (inline brace groups of arms and/or handler values,
see "Handlers as Values"), `BODY` is the handled expression, and the
optional trailing `else (result) => E` transforms the body's value on
normal completion (see "Error handling without exceptions").

Handler arms are fully typed against the ability's declared interface:

- Arm parameters take the method's declared parameter types
  (`FileSystem::read(path)` binds `path: String`).
- `resume(v)` feeds the continuation, so `v` must have the method's
  return type; the `resume(...)` expression itself has the handle
  expression's result type.
- An arm body's own effects (performs outside the handled body) count
  against the enclosing function, like any other code.
- Exception is special: `throw` returns `!`, and the host raises it at
  arbitrary perform sites (see "Host failures are catchable exceptions"),
  so the value passed to `resume` in an Exception arm is deliberately
  unconstrained — it substitutes for the _failing call's_ result, whose
  type is unknowable at the arm.

## Handlers as Values

```ambient
let mock_fs: Handler<FileSystem> = {
  FileSystem::read(path) => resume("mock content"),
  FileSystem::write(path, content) => resume(()),
  FileSystem::exists(path) => resume(true),
};

// Use handler values
with mock_fs, mock_network handle unit_test()

// Override specific methods
with mock_fs, { FileSystem::read(path) => resume("intercepted") } handle unit_test()
```

A handler value binds a brace group of arms to a name. Its type is
`Handler<A, R>`: `A` is the single ability it covers, and `R` is the
_answer type_ — the type an arm yields when it returns _without_
resuming, which is also the result type of any `handle` expression it is
installed on. `Handler<A>` is shorthand for "`R` inferred". Because a
handler value covers exactly one ability, its arm names must be
**qualified** (`FileSystem::read`, not `read`) — the ability is no longer
guessed from method names. A brace group mixing multiple abilities is
allowed only _directly inline_ in a `with` list, never as a `let`-bound
handler value.

Handlers in a `with` list install **left-to-right**, so a later handler
wins over an earlier one for the same method ("last wins" is
_per method_: a handler that does not cover a method is transparent to
it). Above, `with mock_fs, { FileSystem::read(path) => resume("intercepted") }`
installs `mock_fs` first and the inline override second, so
`FileSystem::read` resolves to the override while `write`/`exists` still
fall through to `mock_fs`. A method no handler covers falls all the way
through to its default implementation.

## Sandboxing

```ambient
sandbox with core::system::Log {
  untrusted_code()  // Only the platform Log ability available
}

sandbox {
  pure_untrusted_code()  // No abilities - pure computation only
}
```

The restriction is enforced statically: the body may only _use_ the
allowed abilities, checked at compile time (including through calls to
functions defined elsewhere in the module). The sandbox installs no
handlers — allowed abilities still execute against the enclosing
context's handlers, so the body's effects count against the enclosing
function like any other code.

## The `core::system` module and host bindings

Builtin abilities are not defined in engine code. The engine's only
native ability is `Exception` (part of the language). Everything else —
Stdio, Time, Random, Log, FileSystem, Network, Process, Execute — is declared once, in
Ambient source, in the **platform bindings interface**
(`crates/ambient-platform/src/platform.ab`).

`core::system` is an ordinary module under the reserved `core` root,
resolved through the same `ModuleRegistry` machinery as any other
`core::`/`pkg::` path — no dedicated root or contextual keyword. The
embedder registers it at the path `["core", "system"]`, but the source
still ships in the `ambient-platform` crate, so the engine keeps its
decoupling from any embedder. Its abilities are always in scope
fully-qualified (`core::system::FileSystem::read!`,
`with core::system::FileSystem`, `core::system::FileSystem::read(path) => ...`,
`sandbox with core::system::Log`) with no `use`, and importable by name
(`use core::system::FileSystem;`) to use bare — exactly like `core::` items.
Because ability identity is the declaration uuid, a bare imported
`FileSystem` and a qualified `core::system::FileSystem` share one
`AbilityId`, so handlers, effect rows, and linking unify with no special
casing.

The declarations in `platform.ab` are `pub` for the same reason core
exports are: the registry only imports public symbols, so `use
core::system::FileSystem;` requires `pub ability FileSystem`. Visibility gates
_only_ the bare-import path — fully-qualified use is seeded independently.

The engine seeds the namespaced `core::system::` abilities from the registered
`core::system` module during type checking (`seed_namespaced_ability_dynamics`),
and the general cross-module bridge (`build_import_env`) registers an
imported ability as a bare dynamic — the same code path any
`use pkg::b::SomeAbility;` takes.

The host binding half is the ordinary `extern fn` mechanism — there is no
separate ability-handler channel anymore:

1. The platform module's ability method bodies (its default
   implementations) call module-private `extern fn`s
   (`stdio_out`, `fs_read`, ...). The module **compiles** like any core
   module (`build::compile_system_module`), so those bodies are ordinary
   content-addressed functions that perform sites link against.
2. The embedder binds each extern fn's implementation in a
   `NativeRegistry` under a stable uuid (`BuildOptions::natives` at
   compile time; `Vm::register_natives` at runtime). The uuid — not the
   name — is the binding's identity, so renames re-key loudly and never
   move a hash.
3. An unhandled perform runs the default implementation, whose extern
   call dispatches to the bound native. A VM without that binding fails
   the call loudly (`UnboundNative`) — which is exactly how capability
   granting works for isolated Execute VMs: granting an ability means
   registering its natives.

`ambient-platform` is one such embedder, packaged as a library: it ships
`platform.ab` plus native implementations (std::fs, TCP via tokio, ...)
exposed as `NativeRegistry` constructors (`stdio_natives`,
`network_natives`, `execute_natives`, ... plus contract-satisfying
`stub_natives` for compile-only paths). The engine crate does not depend
on it — another crate can use the engine the same way with entirely
different declarations and bindings. Effectful natives stay unreachable
from user code by ordinary visibility: the extern fns are module-private
to `core::system`, so its ability bodies are their only callers.

## Error Handling

Errors are abilities. `Exception::throw!` raises; the nearest enclosing
`with ... handle` for Exception catches. A handler arm's value becomes the
handle expression's value, and execution continues after the handle
expression (catch-and-continue). The optional trailing `else (result) =>
E` clause transforms the body's value on _normal_ completion only; arms
bypass it.

```ambient
// The one abstract ability method in the language: `throw` returns `!`,
// and its unhandled behavior is the VM's own uncaught-exception path,
// which no in-language default implementation could express.
pub unique(FFFFFFFF-FFFF-FFFF-FFFD-000000000001) ability Exception {
  fn throw(error: String): !;
}

fn parse_int(s: String): Number with Exception {
  match try_parse(s) {
    Some(n) => n,
    None => Exception::throw!("not a number"),
  }
}

// Handling exceptions
fn safe_parse(s: String): Option<Number> {
  with { Exception::throw(e) => None } handle parse_int(s) else (result) => Some(result)
}
```

An uncaught throw halts the program with `uncaught exception: <value>`,
carrying the actual thrown value.

### Host failures are catchable exceptions

Fallible host operations (file not found, connection refused, ...) do not
return `Result` values and do not kill the VM: the native implementation
raises a catchable exception _at the call site_, inside the ability's
default implementation. The calling program catches it like any
in-language throw. Because the Exception handler receives the
continuation of the failed call, it can even `resume` with a substitute
value, and the IO caller continues as if the operation had succeeded
(note: this resume-with-substitute behavior is slated for removal in
favor of plain `Result` returns on fallible APIs):

```ambient
fn fetch_or_default(): Number with core::system::Network {
  with { Exception::throw(msg) => resume(0 - 1) }  // substitute connection id
    handle core::system::Network::connect!(("10.0.0.1", 9))
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
(`FileSystem.read` returns `String`, not `Result<String, _>`) while `handle`
gives callers strictly more power than matching: they can substitute a
fallback for the failing call and continue, not just observe the error
after the fact.
