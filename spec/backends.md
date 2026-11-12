# Backends

Native code and WebAssembly are produced from one lowered form and one runtime. A program behaves identically on both.

## Agreement

- Every observable result is the same on every backend: values, faults, and the order of performs.
- No semantics is defined by a backend; where two backends could differ, the [core tree](core.md) or this page decides.
- The test suite runs every program on every backend and compares the results.

## Lowered form

- The lowered form is derived from a core tree alone and is the only input to a backend.
- It is first-order and structured: every intermediate value is bound to a local, and control flow is nested, never arbitrary jumps.
- [Match](types.md) is lowered to tests and branches; a backend never sees a pattern.
- Reference counting is explicit: the lowered form says where each value is retained and released.
- A backend translates the lowered form one construct at a time and makes no decisions about semantics.

## Representation

- Every value has one representation, whatever its static type, so subtyping never converts or copies.
- A value is one machine word: an immediate, or a reference to a counted object.
- The representation is the same on every backend, bit for bit, so that [encoding](values.md) and equality agree.
- Static types may be used to avoid work, never to change what a value looks like where another definition can see it.

## Memory

- Values are reference counted, and an object is reclaimed when its last reference is released.
- The count is what makes [in-place update](values.md) decidable.
- Reclamation is prompt and deterministic; there is no collector and no pause.
- TODO: a [cell](cells.md) that reaches itself is never reclaimed.

## Runtime

- The runtime is the part of the language that is not compiled from core trees: allocation, counting, structural operations, and shape tests.
- There is one runtime, compiled for each backend; a backend supplies none of its own.
- Compiled code reaches the runtime and other definitions only through tables it is handed, so an [artifact](build.md) is position-independent and never patched.

## Types at run time

- A type a [cast](types.md) or type test names exists at run time as a descriptor: data, laid out when a program is loaded.
- A recursive type definition has one descriptor, which the descriptors that name it refer to.
- No value carries a type but a [cell](cells.md), which keeps the descriptor of the type it was made at; a descriptor is otherwise consulted only where a cast is written.
- A cast to a cell type passes a cell whose descriptor describes the same type as the cast's: names stand for their definitions, and a union is its members in any order.
- TODO: a cell made at a type that mentions a type parameter or a function type keeps no descriptor, and passes no cast to a cell type.
- TODO: a function value carries no signature yet, so a cast to a function type is not compiled.
- TODO: a cast to a type parameter is not compiled; it needs the type argument passed at run time.

## Calls

- Every function is called the same way, whether it is named, a lambda, or a value whose definition is unknown at the call.
- A call through a [slot](core.md) reads the slot at run time, so replacing what a slot refers to recompiles nothing.

## Bindings

- A call hands the callee the caller's [bindings](abilities.md) whole, as one value; the callee reads the abilities in its row from them.
- Passing bindings therefore costs one word per call, whatever the row, and a perform costs one lookup and a call.
- A [provide](handlers.md) makes new bindings for its body: the enclosing ones overlaid with its bundle, or in only-mode the bundle alone.
- Code under an only-mode provide is therefore never handed anything outside the bundle, checked or not.
- A closure holds exactly the bindings it captured, never the whole set they were taken from, so a function value's content does not depend on where it was made.
- A handler for a returning ability is called with no bindings; what it needs was bound where it was provided.

## Suspension

- A [task](tasks.md) suspends by unwinding: each function on its stack saves what it holds and returns, and resuming calls them again from the outermost, each going back to where it was.
- Suspension is therefore ordinary control flow in the lowered form, like unwinding, and needs nothing from a backend or a host: no second stack and no thread.
- A suspended task is data on the heap, owned by the task.
- Cancelling a suspended task resumes it into an unwinding, so its [cleanups](cleanup.md) run as they do on any other exit.
- A suspension costs in proportion to the depth of the stack that suspends; code that never suspends pays only the check it already makes after each call.
- The runtime calls compiled code only to start or resume a task.

## Unwinding

- Unwinding is compiled as ordinary control flow: every call yields a value or a signal to unwind, and the caller checks.
- A [fault](values.md), an [abort](handlers.md), and a cancellation all unwind this way.
- No backend's exception mechanism is used, so unwinding behaves identically everywhere and [cleanups](cleanup.md) are ordinary code.
- A provide binds each abort handler to a token of its own; a perform unwinds to the provide that holds the token, and every provide it passes lets it through.
- A token expires when its provide exits, however it exits; performing through an expired token is the [fault](handlers.md).
