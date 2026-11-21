# Externs

An extern is a definition implemented in native code. Ambient declares the interface, and a native bundle supplies the implementation by the extern's [identity](definitions.md).

## Kinds

- An extern function is a native function, pure but for the abilities of its row; callers cannot tell it from an Ambient function.
- A native handler is a native implementation of an [ability](abilities.md).
- A native handler cannot be called; it can only be a root binding in a [target](targets.md).
- Effectful native code is therefore reachable only by performing an ability, never by knowing a hash.
- An extern function's purity is its author's contract and is not verified.

## Declaration

- An extern is declared in Ambient with a full signature, and is checked like any other header.
- A native handler names the ability it implements and takes that ability's signature. One for an ability that does not return is handed the [trace](errors.md) of the abort after it.
- An extern may declare a row of abilities it performs, and whatever calls or binds it must cover that row as it does any function's.
- Native code is handed bindings for its declared row alone. One whose row says too little finds nothing bound for the rest, so a wrong row fails and grants nothing.
- A native handler that may suspend is declared `suspending`; one that is not never does.
- An extern with no native counterpart for the selected target is a link error only if the entry reaches it.

## Native side

- Native code can construct any [value shape](values.md), mint handles, and call function values it is given.
- Native code performs the abilities in its declared row with the same bindings Ambient code would get.
- Native code reaches its program only when called. It has no initialisation, and what it leaves working can do one thing to the program: answer a wait.
- Native code is handed its arguments and the bindings of its caller, and gives back a value or raises a [fault](values.md).
- Native code is also handed the program that called it, and can [load](backends.md) a function into it; what it gets back is that function as a value.
- A suspending native handler may suspend the [task](tasks.md) that performed it while it waits on the host.
- It suspends by saying it will answer later. Other tasks run, and the perform ends when the host gives the answer: a value, or a fault.
- Whatever does the waiting is outside the program: the host's, or a thread the native code keeps. The host hands the answer over when it next calls in.
- Only the program's own thread touches a [value](values.md). A thread native code keeps holds none, and gives its answer as what the host can make a value of.
- A task cancelled while it waits leaves the wait. The host is told it may stop, and an answer that comes anyway is dropped.
- A thread native code keeps may ask whether its task waits still. What it has taken of the world for a task that left is its own to keep for the next: nothing gives it back.
- A handle's type is a [brand](types.md) declared alongside the extern that mints it.

## Calling back

- Native code performs an ability of its declared row by calling the handler bound for it, as it calls any function value it was handed.
- Native code therefore composes as Ambient code does: one that reads a file performs the ability, and never reaches for its host's own way to.
- A call back belongs to the [task](tasks.md) that called the native code, so nothing runs that a task of the program did not start.
- A function type among an extern's parameters has an empty row. What is handed over was bound where it was handed, by the [capture](abilities.md) rule, so native code holds bindings only for its own row.
- Native code calls only a function it was handed for the call it is in. What it reads of a function is a copy, which holds no [cell](cells.md) and no handler the function captured.
- What native code calls may suspend, as any call may. Other tasks take their turns beneath the native code until the task can go on, and the native code sees only that the call took a while.
- The world cannot answer while native code stands: a host hands answers over from outside every call. A task that can go on only when the world answers, beneath native code, [faults](values.md).
- A suspending extern that calls what may wait on the world therefore calls back by asking: it says what to call and that it will go on later, the task makes the call as its own, and the host is handed what the call gave.
- Nothing native is on the stack while that call runs, so it may wait on anything.
- A call back that unwinds, by a fault, an [abort](handlers.md), or a cancellation, unwinds the native code that made it: the native code lets go of what it holds and cannot answer.
- Native code that calls back counts toward the [depth of calls](backends.md#calls) at a fixed size, so calls nested through native code fault at the limit as any others do.
- Native code names an ability as an extern is named: by its package and its module path and name.
- TODO: a suspending extern cannot ask yet, so native code that performs what waits on the world faults.
- TODO: a function type reached through a named type is not held to an empty row.

## Linking

- A program lists the externs its entry reaches; whoever loads it supplies native code for each, found by its identity, before it runs.
- Compiled code names an extern by its position in that list, never by address, so an [artifact](build.md) is the same whatever native code it is loaded with.
- A program that reaches an extern nobody supplied is not loaded.
- A program's native code is fixed when it is built. Nothing native is [loaded](linking.md#loading) into a running program, and none is [sent](exchange.md).
- A program therefore comes by a new extern only by being built again, while its Ambient code is replaced as it runs.

## Native bundles

- A native bundle is machine code for one platform: a function for each extern it implements, found by the extern's identity.
- A native bundle is [linked](linking.md#native-bundles) against the runtime as the runtime's own parts are, so its functions take what the runtime hands native code and give what it expects back.
- A native bundle makes and reads [values](values.md) where they lie, through what the runtime gives any host. Nothing is copied to cross into native code.
- What a native bundle is written against is therefore C's way of calling and nothing of one language. A library over it in a language is a convenience, and no native bundle needs one.
- WebAssembly is a platform like any other: a native bundle for it is [linked into the module](targets.md#webassembly-hosts).
- Host code is what a native bundle is called for a platform whose language is not compiled, as JavaScript: it is carried as source, a function for each extern.

## Distribution

- A native bundle is distributed compiled. Building a program links native bundles and runs no code of the package they came with, so nothing a package does to make one happens on the machine of whoever depends on it.
- A native bundle is a [file named by its digest](packages.md#layout), which the manifest lists for a platform. A [snapshot](packages.md) therefore fixes the native code of every platform without holding any, and a [lock](releases.md#locks) says nothing of platforms.
- A package has one native bundle to a platform. What a program lacks of it costs nothing, so a package is divided only to divide what is fetched.
- A native bundle changes only when its package's snapshot does. Nothing a package brings is run to fetch one, to link one, or after.
- A [build](tools.md#builds) reads only the native bundle for the platform it is for, and only if its entry reaches an extern of the package.
- Only externs the entry reaches are linked into a program.
- A package brings native code only where what declares it [says it may](packages.md#grants).
- Definition hashes do not cover native code; a package's snapshot does.
- TODO: a [comptime](comptime.md) call and a [test](packages.md#testing) reach no native bundle, since what runs them was linked before the package was known.
- TODO: a program that packs or compiles code reaches no native bundle, since what loads it is the compiler's own executable.
