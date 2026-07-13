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
- Trait bounds on a method's type parameters (`fn pick_equal<T: Eq>`) are
  part of the canonical signature (rendered by the _spelled_ trait name,
  like unresolved cross-module nominals): adding or removing a bound
  re-keys the method. At runtime the bound's dictionary rides as a hidden
  trailing perform argument and the default implementation binds it — see
  [Generic Constraints in traits.md](traits.md#generic-constraints).
  A handler arm may cover a bounded method: the arm checks in a fresh rigid
  scope with the method's type parameters rigid and its bounds installed as
  the arm's dictionary context, so a bound-method call inside the arm
  resolves to a `DictSource::Param` that forwards the dictionary the perform
  delivered (`check_handler_arm` in `infer/effects.rs`). The arm's own
  dictionary constraints are isolated and solved before the enclosing body's.

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

The carve-out is **never-returning methods** (`: !`), which may omit the
body and stay abstract — an unhandled perform is then a runtime fault
(the uncaught-exception path for `Exception::throw`, an
unhandled-ability error otherwise). See "Never-returning methods" below.

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
  fn pick_equal<T: Eq>(a: T, b: T): Bool {   // trait bounds work here too:
    a.eq(b)                                  // same rules as any function
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

The platform abilities (Stdio, FileSystem, Tcp, ...) are themselves plain
`ability` declarations — see "The `core::system` module" below. Handlers
override defaults: `with ... handle` expressions and handler values
intercept a perform before the default implementation runs. Abilities
import across modules like any other item: `use
pkg::b::SomeAbility;` (and `use core::system::Tcp;`) brings the ability into
scope under its bare name, and every ability is also reachable fully
qualified with no import (`with pkg::b::SomeAbility`,
`pkg::b::SomeAbility::method!(…)`) — the same rule as every other item.
This holds in the REPL too: it registers `core::system` and every project
module, so `use core::system::…` and `use pkg::b::SomeAbility;` work there,
as does an `ability` declared in one REPL turn and used in a later one.

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
  with core::system::Tcp, Log
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

An **ability variable** (`E!`) quantifies over an effect row rather than
a type. `with E` in a signature is the row's polymorphic tail: `map`
requires exactly the effects its `f` performs, whatever they are.

Ability **methods** are effect-polymorphic on the same footing as
ordinary functions — the platform abilities use this to type their
function-valued parameters precisely:

```ambient
ability Task {
  // A task body is a zero-parameter, effect-polymorphic function.
  fn ensure<E!>(name: String, body: () -> () with E): () { ... }
}
```

`E` appears only in **parameter** position here, so it does not join the
method's own effect row: a task body's effects run in the task's own VM
against default implementations, never against the performer's handler
stack, so `Task::ensure`'s contract stays empty. The checker still uses
the row to reject a non-function or wrong-arity body up front. `State`'s
`init`/`update`/`init_versioned` type their `make`/`migrate`/`f`
parameters the same way (`() -> S with E`, `(S) -> S with E`, ...).

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
- Arms for never-returning methods (`Exception::throw`, any method
  declared `: !`) are **catch-only**: the perform site unwound, so there
  is no continuation and `resume` is a dedicated type error — the arm
  yields a value directly (catch-and-continue). There is no way to
  substitute a value for a failing call and continue; fallible host
  operations return `Result` and are matched on instead (see "Fallible
  host operations return Result").

## Never-returning methods

A method declared `: !` never resumes its caller — performing it
**unwinds**. Three things follow, and together they make `!` a
first-class part of the ability system rather than an `Exception`
special case:

- **Checking**: a `!`-typed expression fits any context (bottom
  elimination — the value can never exist, so the use site is
  unreachable). `if (ok) { n } else { Exception::throw!("...") }` is a
  `Number`, a throw can be a typed function's tail expression, a match
  arm, or a `resume` argument's sibling. The encoding is `∀a. a`: the
  perform adopts a fresh type variable. Introduction stays strict — only
  declared signatures produce `!`, so `fn lie(): ! { 42 }` is still a
  type mismatch.
- **Runtime**: the VM discards the delimited computation at the perform
  (stack segment, frames, and the handler entries delimiting them) and
  runs the arm with no continuation — the arm's value lands at the
  handle expression's completion point, exactly like a non-resuming arm,
  but nothing was captured or retained.
- **Declaration**: the method may omit its default implementation
  (`fn abort(code: Number): !;`) — there is often nothing sensible for
  an unhandled perform to run. An unhandled abstract perform is a
  runtime fault: the uncaught-exception error for `Exception::throw`,
  an unhandled-ability error for anything else. A never method may
  still provide a body (e.g. one that translates into another never
  ability it declares in its `with` row).

### Suspended ability values with never results (design note)

No surface syntax currently produces a first-class _suspended_ ability
operation: a bare `Exception::throw("x")` (no `!`) does not parse as
"suspend without performing", and the compiler always emits
`Suspend`+`Perform` as a pair, so `Type::AbilityValue` is unreachable
from source today. If/when ability-value syntax lands, never methods are
specified to behave as follows:

- **Typing**: bottom elimination applies at the **perform site**, not
  the binding site. Binding a suspended never operation has the ability
  value's own type (`AbilityValue<!, E>` — a perfectly ordinary value);
  _performing_ it adopts the context exactly like a direct
  `Ability::method!(...)` perform does.
- **Runtime**: already correct. The `never` unwind flag travels inside
  the suspended value itself, and the value's only constructor derives
  it from the method's compiled reference (`SuspendedAbility::
from_method_ref`), so a stored, passed-around, or host-built suspended
  never operation unwinds at its eventual `Perform` exactly like a
  compiled perform site (pinned by a VM test:
  `test_host_constructed_suspended_never_value_unwinds`).

## Handlers as Values

```ambient
// `read`/`write` are fallible, so `resume` feeds their `Result` return
// type; `exists` is infallible (`Bool`).
let mock_fs: Handler<FileSystem> = {
  FileSystem::read(path) => resume(Ok("mock content")),
  FileSystem::write(path, content) => resume(Ok(())),
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
Stdio, Time, Random, Log, FileSystem, Tcp, Process, Execute, Live,
State —
is declared once, in Ambient source, in the **platform bindings
interface** (`crates/ambient-platform/src/platform/`).

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
`tcp_natives`, `execute_natives`, ... plus contract-satisfying
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
// which no in-language default implementation could express. It carries an
// arbitrary `E: Show` (see "Typed exceptions" below) — a message, a record,
// an enum — so an uncaught handler can always render it.
pub unique(FFFFFFFF-FFFF-FFFF-FFFD-000000000001) ability Exception {
  fn throw<E: Show>(error: E): !;
}

fn parse_int(s: String): Number with Exception {
  match try_parse(s) {
    Some(n) => n,
    None => Exception::throw!("not a number"),   // E = String
  }
}

// Handling exceptions. One polymorphic arm catches every `E`; `e` is opaque
// except through its `Show` bound (a rigid `E` has no known shape).
fn safe_parse(s: String): Option<Number> {
  with { Exception::throw(e) => None } handle parse_int(s) else (result) => Some(result)
}
```

An uncaught throw halts the program with `uncaught exception: <value>`,
carrying the actual thrown value.

### Fallible host operations return Result

Fallible host operations (file not found, connection refused, ...) return
a `Result<T, String>`: the native yields an in-language `Result::Err(message)`
value the caller matches on, exactly like any other data. They do not
raise exceptions and do not kill the VM.

```ambient
fn fetch_or_default(): Number with core::system::Tcp {
  match core::system::Tcp::connect!(("10.0.0.1", 9)) {
    Ok(conn) => conn,
    Err(msg) => -1,          // substitute connection id
  }
}
```

The migrated platform methods — `FileSystem::read`/`write`/`list`/... ,
every `Tcp` method, `Stdio::read`, and `Env::cwd` — carry `Result`
return types in `platform.ab`; their default implementations forward the
`Result` the native produces. Infallible operations keep their bare types
(`FileSystem::exists` returns `Bool`, `Stdio::out` returns `()`).

Two things still travel the catchable `Exception` channel, and both are
hard failures — nothing resumes them:

- An **unwired capability**: performing an ability whose native is bound
  to the "not wired" stub (e.g. an ungranted ability in an isolated
  Execute VM) throws `... is not wired`, surfacing uncaught unless a
  program deliberately handles it.
- A **control error** a native can only detect at runtime, like ensuring
  a task with a body that is not a zero-parameter function.

Engine-level faults (stack overflow, type errors in bytecode, arity
mismatches) remain fatal `VmError`s — they indicate bugs, not conditions
programs should handle.

### Typed exceptions: `throw<E: Show>`

`throw` carries an arbitrary `E: Show`, not just a string. Because ability
methods take type-parameter bounds and handler arms bind their dictionaries,
this was a small, incremental change rather than a new subsystem.

**Decision.** Generalize the _method_, not the ability:

```ambient
pub unique(FFFFFFFF-FFFF-FFFF-FFFD-000000000001) ability Exception {
  fn throw<E: Show>(error: E): !;
}
```

The ability stays non-generic and singular. There is exactly one
`Exception` identity, one handler channel, and one effect-row entry
(`with Exception`) — a generic _ability_ `Exception<E>` is rejected on two
counts: trait/ability-level type parameters are not supported (see
[traits.md](traits.md#defining-traits)), and effect rows key on
`AbilityId`, so `with Exception<E>` has no representation in row
unification and would fracture "one handler catches every throw" into one
handler per `E`. Generalizing the method keeps every mechanism that
already exists and adds only a bound.

**1. Anchor.** Ability identity is `EXCEPTION_UUID`
(`ambient_core::exception::ability_id()`) and is untouched — the uuid does
not mention the signature. Method identity is not: `throw`'s `MethodKey`
is `MethodKey::derive(EXCEPTION_UUID, throw_signature(), None)`, and
`throw_signature()` changed from `(string) -> never` to the generic
canonical form `<E: Show>(E) -> never` (rendered `var0` + the `bound:0:Show`
pseudo-parameter — type variable numbered by first occurrence, bound
rendered by the spelled-name convention). So `throw` **re-keyed**. That is
acceptable and mostly invisible:

- The uncaught path in `op_perform` keys on `ability_id` alone
  (`ability.ability_id == exception::ability_id()`), not on the method
  key, so it is unaffected.
- `raise_exception` (the host/native throw path) recomputes
  `throw_method_key()` from the anchor, so it tracks the new signature for
  free.
- The checker/compiler special cases that recognize Exception key on
  `EXCEPTION_UUID` (the `STATE_UUID` fingerprint carve-out is the
  precedent), never on the method key, so none of them move.

The re-key does invalidate content hashes of code compiled against the old
`throw` — a store/deploy concern, not a soundness one. With no
backwards-compatibility constraint this was a one-time reset: the
`throw_signature()` bump regenerated the golden tests that pin it
(`ambient-cli/tests/platform_prelude.rs`, `ambient-core/src/exception.rs`,
and the prelude set in `ambient-cli/tests/core_prelude_enums.rs`). Never
renumber `EXCEPTION_UUID` itself.

**2. Ergonomics.** With one polymorphic method, a single arm catches every
`E`:

```ambient
with { Exception::throw(e) => handle_it(e) } handle body
```

The arm binds `e: E` in a fresh rigid scope with the `Show` dictionary
forwarded as `DictSource::Param` (exactly the bounded-method arm path
described under Ability Identity). Inside the arm `e` is opaque except
through `Show`'s methods (`e.show()`) — it cannot be inspected structurally,
because a rigid `E` has no known shape; the dictionary is the only handle on
it.
`Result`-interop is the existing catch-and-continue plus `else`, with no
new `try` keyword:

```ambient
// body: T with Exception  ⇒  Result<T, SomeError>
with { Exception::throw(e) => Err(e) } handle body else (r) => Ok(r)
```

For a caller that wants a concrete error type it annotates it; the arm's
rigid `E` unifies with the annotation at the `handle` site.

**3. The `Show` bound.** `E` is bounded so an uncaught (or logging)
handler can always render it. **Decision: `Show`, not a distinct `Error`
marker.** The bound exists solely so a value can be turned into a string;
`Show { fn show(self): String }` is the general, reusable, conventional
minimum. A dedicated `Error` trait (`fn message(self)` plus, say, a cause)
would add ceremony without behavior the language needs today — and a program
that wants structured errors already has a better tool: throw a concrete
enum and match on it after catching (the polymorphic-throw / monomorphic-
catch asymmetry below). If structured errors ever earn their own trait, it
is an additive change, not a migration. `Show` claims the slot after
`Default` in the reserved trait block (`FFFF…-0010` holds `Add`…`Default` at
`-0017`, so `Show` takes `FFFF…-0018`) and is validated by
`validate_reserved_trait` like the operator traits. It is re-exported onto
the prelude next to the operator traits so `throw` and its bound are always
in scope bare, and `core::traits` provides `impl Show` for `String` (renders
as itself), `Number`, `Bool`, and `Binary` (via the host `to_string`).

**4. Uncaught path.** `VmError::Exception(value)` already renders through
`format_value`, which walks _any_ `Value` structurally (records, enums,
lists, primitives) — so an arbitrary `E` already prints without a
dictionary at the crash site, and no dictionary is available there anyway
(the value has outlived every frame). The structural rendering is the
runtime floor (verified by `test_uncaught_record_exception_renders_
structurally`). The `Show` bound is a compile-time _refinement_: where the
thrown expression's static type is known, the compiler could emit
`E::show(value)` before the throw and carry the rendered string alongside
the value, so the uncaught message reads as the type intends rather than
as a raw record dump. **This refinement is not implemented** — structural
fallback holds, and threading a pre-rendered string through the
perform/unwind path (a new channel on `VmError::Exception`) is out of scope
for the initial landing. See step 4 of the sketch.

**5. Arm typing.** One polymorphic arm, not per-instantiation arms. The
arm is checked once with `E` rigid and the `Show` dictionary in scope
(`check_handler_arm`), and it covers every throw regardless of the
concrete `E` at each perform site — the same shape a bounded free function
gets. There is no per-`E` handler table; dispatch is still the single
`(EXCEPTION_UUID, throw_method_key)` match.

**6. What stays a string.** `throw("msg")` keeps working unchanged: with
`impl Show for String` in `core::traits`, the call infers `E = String` and
builds the `String` dictionary at the site like any bounded call — no sugar,
no special case, no deprecation step. A bare `throw(record)` works the
moment `record`'s type implements `Show`.

**Implementation sketch** (each step landed independently):

1. Add the `Show` trait to `core::traits` with the reserved
   `FFFF…-0018` uuid, `validate_reserved_trait` entry, and prelude
   re-export; `impl` it for `String` and the primitives. (Pure trait work,
   no Exception change yet.)
2. Change `throw_signature()` to the generic form and update
   `core_lib/exception.ab` to `fn throw<E: Show>(error: E): !`. Regenerate
   the golden tests pinning the signature/method key. `throw("msg")` now
   compiles through the dictionary path. **Done.**
3. Confirm the uncaught path and `raise_exception` need no change (they key
   on the uuid / recompute the key); add a regression test throwing a
   record and asserting structural rendering. **Done.**
4. (Optional refinement, **not done**) At compiled `throw!` sites with a
   statically known `E`, emit `E::show` and thread the rendered string into
   `VmError::Exception` for prettier uncaught output. Deferred: it needs a
   new channel on the exception value threaded through the perform/unwind
   path, and structural rendering (step 3) is the specified floor.

The one genuinely awkward spot: a rigid `E` in a catch arm is opaque, so
"catch, then branch on which error it was" wants either the `Show` trait
to expose enough (a tag/`show`) or the program to throw a concrete enum
and match after catching — the polymorphic-throw / monomorphic-catch
asymmetry is inherent to a single-ability, single-arm design and is the
price of not making `Exception` generic.

### Option/Result vs exceptions

`Option` and `Result` are ordinary data types for _domain modeling_: a
lookup that may find nothing returns `Option`, a parser that produces a
structured error returns `Result`. They are values you match on.

_Operational failure_ — the file was deleted, the peer hung up — is also
modeled as data: a fallible platform method returns `Result<T, String>`
(`FileSystem::read` returns `Result<String, String>`), and the caller
matches on `Ok`/`Err` like any other value. Exceptions are reserved for
_faults_ the program should not routinely recover from — an unwired
capability or a runtime control error — and are catch-only: a handler can
observe the failure and continue past the `handle` expression, but cannot
resume the failing operation with a substitute value.
