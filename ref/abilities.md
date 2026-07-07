# Abilities

Part of the [Ambient Language Reference](architecture.md).

Abilities are the mechanism for controlled side effects.

## Ability Identity

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

## Declaring Abilities

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
`ability` declarations — see "The `core::system` module" below. User abilities
are handled in-language (`with ... handle` expressions or handler values); a performed
ability with no handler in scope — in-language or host — is a runtime
error. Abilities import across modules like any other item: `use
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

Every module's abilities are in scope *fully-qualified* (`core::system::Stdio`
or `pkg::effects::Counter` in performs, `with` clauses, effect-row
annotations, handler arms, and sandbox clauses) with no `use` — the same
rule as every other item. To drop the prefix, import the ability:
`use core::system::Stdio;` then `with Stdio` and `Stdio::out!(...)` work
bare thereafter. A bare `Stdio` that was *never* imported (and is not a
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
fn add(x: number, y: number): number { x + y }
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
*answer type* — the type an arm yields when it returns *without*
resuming, which is also the result type of any `handle` expression it is
installed on. `Handler<A>` is shorthand for "`R` inferred". Because a
handler value covers exactly one ability, its arm names must be
**qualified** (`FileSystem::read`, not `read`) — the ability is no longer
guessed from method names. A brace group mixing multiple abilities is
allowed only *directly inline* in a `with` list, never as a `let`-bound
handler value.

Handlers in a `with` list install **left-to-right**, so a later handler
wins over an earlier one for the same method ("last wins"). Above,
`with mock_fs, { FileSystem::read(path) => resume("intercepted") }`
installs `mock_fs` first and the inline override second, so
`FileSystem::read` resolves to the override while `write`/`exists` still
fall through to `mock_fs`.

## Sandboxing

```ambient
sandbox with core::system::Log {
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
Because ability identity is the content-addressed interface hash, a bare
imported `FileSystem` and a qualified `core::system::FileSystem` share one
`AbilityId`, so handlers, effect rows, and linking unify with no special
casing.

The declarations in `platform.ab` are `pub` for the same reason core
exports are: the registry only imports public symbols, so `use
core::system::FileSystem;` requires `pub ability FileSystem`. Visibility gates
*only* the bare-import path — fully-qualified use is seeded independently.

The engine seeds the namespaced `core::system::` abilities from the registered
`core::system` module during type checking (`seed_namespaced_ability_dynamics`),
and the general cross-module bridge (`build_import_env`) registers an
imported ability as a bare dynamic — the same code path any
`use pkg::b::SomeAbility;` takes.

An embedder still wires the **host binding** half:

1. Parse the declarations and resolve them
   (`resolve_ability_declarations`) into content-addressed interfaces, and
   register the `core::system` module in the registry
   (`register_declaration_module`) so the naming layer resolves it.
2. Pass the resolved interfaces in `CompileOptions::prelude_abilities` for
   compilation. Type checking no longer needs an embedder ability resolver:
   performs resolve against the seeded `core::system` module, the same path
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

## Error Handling

Errors are abilities. `Exception::throw!` raises; the nearest enclosing
`with ... handle` for Exception catches. A handler arm's value becomes the
handle expression's value, and execution continues after the handle
expression (catch-and-continue). The optional trailing `else (result) =>
E` clause transforms the body's value on _normal_ completion only; arms
bypass it.

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
  with { Exception::throw(e) => None } handle parse_int(s) else (result) => Some(result)
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
fn fetch_or_default(): number with core::system::Network {
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
(`FileSystem.read` returns `string`, not `Result<string, _>`) while `handle`
gives callers strictly more power than matching: they can substitute a
fallback for the failing call and continue, not just observe the error
after the fact.
