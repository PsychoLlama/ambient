# Targets

A target defines the world a program runs in: a root binding for every ability that world offers. Compiling for a platform, embedding in a host, running a build, and running tests each amount to choosing a target.

## Root bindings

- A target is a definition that maps abilities to handlers; each handler is a [native handler](externs.md) or an Ambient function.
- A program is an entry function and a target.
- A program runs as its entry under a restricted [provide](handlers.md) of its target.
- The entry's row must be covered by the target.
- A root handler's own row is satisfied by the target's other bindings; a cycle among them is an error.
- Root handlers are therefore provided in layers, each over the handlers its rows need, and only the outermost is restricted.
- A root handler for an ability that does not return ends the program, and its result is the program's.
- An entry has no type parameters; its parameters are the program's.
- No ability has an implementation outside a provide; a target is only the outermost one.
- A target also fixes how [tasks](tasks.md) are scheduled.

## Composition

- A target may extend other targets, adding and replacing bindings; a conflict between extended targets is resolved explicitly.
- Libraries export abilities and handlers for them; targets select among them.
- A program runs unchanged on any target that covers its row.
- Portability is a property of rows, not of conditional compilation.

## Pruning

- A root binding the entry's row does not reach is neither compiled nor linked.

## Embedding

- An embedder defines a target from the host language and supplies native handlers directly.
- An embedder's target need not include any standard binding; a program is then limited to pure code and what the embedder provides.

## Calls from the world

- The world never calls a program. Whatever a world would call a function for, a task waits for instead, through an ability that suspends.
- An event, a request, and a timer are each such a wait, and what handles one is the task that waited: nothing runs outside the [tasks](tasks.md) the program started.
- What a task waits on is a listener, a [handle](values.md) the world holds. Its next call is the call's arguments and, where the world wants an answer, a handle to answer by.
- Giving a function to be called for each event is library code: a group that waits, and starts a task for each call.
- A host that owns the thread hands a call over and runs the program until every task is suspended, before it returns to its world.
- What a task does before it next waits is therefore done inside the event that woke it, which is when a world takes an answer or lets an event be cancelled.
- A listener holds a bounded number of calls nothing has taken. One that is not taken inside its event gets no answer there.
- A listener nothing holds stops listening.

## The system package

- The standard library, `ambient`, holds what every target provides, and nothing in it needs a world: the shapes' methods, maps, JSON, errors, cells, cleanup, and what a test checks with.
- The `system` package holds the abilities of a program on a machine: its streams, environment, files, clocks, commands, and in time the network, randomness, and cryptography.
- It says what each ability means and promises no target binds it: a target binds whichever it can.
- A browser's target binds fetching and not serving; an embedder's binds its host's own abilities beside whichever of these the host can do.
- Its ID is fixed, so every target means the same ability by `system.fs.read`, and a library written against one runs under any target that binds it.
- It comes with the compiler, as its native code does, and a package reaches it under the name `system` without declaring it.
- A host may be built without it; a program is then limited as any [embedder's](#embedding) is.

## Standard targets

- The `system` package defines a target for a program run from a command line: its streams, its arguments and environment, files, clocks, random bits, commands it runs, and its exit.
- Each of those is a module of abilities, so a row says which of them a function reaches.
- An operation on a file that can fail in ordinary use gives an [error](errors.md) as a value; the kinds are tags of the module.
- A file is read, written, and added to as text or as [bytes](operations.md#bytes), each by an ability of its own. Reading as text a file that is not UTF-8 is an error of a kind of its own.
- Randomness is one ability, bits nothing could have guessed; every other draw is a function over it, so a provide of that one ability decides them all, and the package has one that follows from a seed.
- Exiting is an ability that does not return, so the program's result is the code it exits with.
- Being asked to stop from outside is a [call from the world](#calls-from-the-world): a task waits for it, and is told what was asked. A program no task of which has waited is ended by being asked, as any process is; once one has, stopping is the program's to do, so a daemon leaves cleanly.
- Running a command gives what it exited with and what it wrote; no shell reads the command, and one that exits with a code that is not zero has still run.
- A command is a record: the program, its arguments, the directory it runs in, variables added to its environment, and what it reads from its input. A function makes one from the first two, to be copied with whatever else is wanted.
- What a command reads and what it writes are bytes, as a file's are: the command decides what its streams hold.
- Nothing the world gives is altered to make it text. A file, a response, and a command's streams are bytes; an argument, a variable, and a name in a directory must be text.
- Standard input is read a line at a time as text, or as bytes, up to a count at a time; each reading takes up where the last left off.
- A program started with an argument that is not text is not run; a variable that is not text is not among its variables; a directory that holds a name that is not text is not listed, which is an error of kind `invalid`; a header of a response that is not text is not among its headers; a line of standard input that is not text ends the program, as a failure of its native code does.
- A command that could not be started is an error whose kind is a tag of that module, never one of the files'.
- What takes time there suspends its [task](tasks.md) and blocks nothing: tasks sleep, run commands, and wait for input together, and a sleep beside other work in a first is a limit on it.
- Work whose task was cancelled is stopped where it can be: a command is ended, and a line already being read is kept for whoever reads next.
- Both standard targets bind `throw`: an [error](errors.md) nothing caught ends the program, or fails the test, and is reported as a fault is.
- The standard library defines a target for a [test](packages.md), which binds a check and nothing of the world, so it needs no `system`.
- The build target and test targets are ordinary targets; see [comptime](comptime.md) and [memoization](memoization.md).

## WebAssembly hosts

- A WebAssembly program is a module that imports its [native code](externs.md) and exports what its host calls. It assumes nothing else of where it runs.
- Any host that supplies those imports runs it, and owns the thread: tasks run only while it [calls in](tasks.md).
- No host is the standard one, so a program built for one carries nothing of another.
- Native code reaches such a program two ways. What only a host can do is the host's, in the host's language; what is only computation is itself WebAssembly, [linked](linking.md) into the program as the runtime is.
- Linked native code shares the program's memory and is carried only if reached, so it runs under every host and reads a value where it lies.
- Making a program into what a host loads is library code: the [tool](tools.md#builds) its target names, which gives the files.
- A target with no such tool is built as the module alone, for a host that loads it by hand.
- TODO: nothing links native code into a module.

## JavaScript hosts

- A browser's target and Node's are packages: their abilities, the bindings of whichever [`system`](#the-system-package) abilities each can do, and the JavaScript behind both.
- The compiler comes with neither. What it knows of JavaScript is nothing.
- Such a package's host code is JavaScript: a function for each extern, found by its identity.
- Building under one gives the module and the JavaScript that loads it, with a function for each extern the program reaches and no others. What a page fetches follows from what its entry reaches.
- How an extern's values cross is JavaScript the target's tool writes, for each extern reached, from its signature. No package writes that by hand, and none pays for a conversion its signature does not need.
- An extern may say by an [annotation](annotations.md) what of JavaScript it stands for: a function, a method, a property. The tool then writes its function as well, which is how bindings are made from a description of an interface.
- The tool also gives the types of what the host calls and is handed, as a declaration JavaScript's tools read.
- A library declares an extern once, for every host. Another host's target reads the same signatures and its own annotations, so supporting it changes no library that only declares.
- A suspending extern's function gives a promise, and what the promise settles to is the answer.
- A function among an extern's arguments is a JavaScript function, which [calls back](externs.md#calling-back) while the extern's function runs and throws after.
- An extern's function is handed the call it is in after its arguments, and performs the abilities of its row through that.
- Code is [loaded](linking.md#loading) there between a program's turns, never beneath native code: loading gives a promise of the function, so the ability that loads is one that suspends.
- A loaded function the host holds keeps its code; answering with it hands it to the program, and code nothing holds is dropped.
- A handle minted there stands for a JavaScript object. The host keeps the object by the handle and lets it go when the handle is reclaimed.
- An event of a page is a [call from the world](#calls-from-the-world), so a handler that cancels one does so before it next waits.
- TODO: one package is the base of such targets, and none is a browser's or Node's. An extern with host code of its own is taken whole with the file it is in, and every value is converted as it crosses.
- TODO: how JavaScript sees a value is undecided. Converting each at the call copies it, so a value is more likely held as it is and read through the program, with converting asked for.

## Network

- `system.http` sends a request and gives the response, each whole: a method, a URL, headers, and a body out; a status, headers, and a body back.
- A response is a value whatever its status. Only a request that got no response is an [error](errors.md).
- A server is a [listener](#calls-from-the-world). Accepting its next request is an ability that suspends, and answering one is another.
- The way to answer a request is a handle too. A request nothing can answer is let go, which ends its connection.
- Serving is library code over those: a group that accepts, and starts a task for each request. What answers is an ordinary function, and leaving the group ends the serving.
- Nothing takes a timeout: a sleep beside the work in a first is one.
- A header's name is lower case wherever the world gives one.
- A body is [bytes](operations.md#bytes), so anything can be sent and received. Text and JSON are put into a body and read out of one by functions.
- TODO: a body is held whole; nothing is streamed.
- A server holds a bounded number of requests nothing has accepted; one sent past that is refused as unavailable, and the program never sees it.

## Streams and sockets

- A stream is a [handle](values.md) to bytes that come and go in order: a connection, or a body still arriving. `system.stream` reads one, writes one, and closes one, each an ability.
- Reading waits for what the stream has next and gives at most what was asked for, or nothing once the stream has ended. Writing sends all it is given, in order.
- Bytes read for a task that was cancelled meanwhile are kept, and the next to read is given them.
- A stream nothing holds is closed.
- `system.net` is what is below HTTP: it connects to a port of a host, or to a Unix socket, and gives a stream.
- A listener there is a [listener](#calls-from-the-world) whose next call is a connection, a stream. It holds a bounded number of connections nothing has accepted, and one made past that is ended.
- A datagram socket is bound to a port, sends a datagram to an address, and waits for the next it was sent, which says where it came from. It holds a bounded number nothing has received, and one that arrives past that is dropped, as a network may drop any.
- A socket joins a multicast group to receive what is sent to it. A port bound as shared may be bound by other programs too, which is how several listen to one group.
- Protocols over these are packages: nothing here knows of one.
- TODO: nothing encrypts a stream; only `system.http` speaks TLS.

## Code

- `system.code` packs a function, compiles a pack, and loads what was compiled: the halves of [exchange](exchange.md), each an ability.
- Packing gives a [pack](exchange.md) of a function and everything it reaches, as bytes. It leaves nothing out, so whatever has the compiler can compile it.
- Compiling rechecks a pack and gives code for the program that asked, as bytes. Bytes that are not a pack, and a pack checking could not have made, are each an [error](errors.md).
- Loading adds compiled code to the program and gives the function as `unknown`; a [cast](types.md) to the type expected is the check that it fits.
- Compiled code is trusted as the program is: nothing checks it, so a program loads only what it compiled or kept itself. Holding the ability to load is holding that trust.
- Compiled code says which build of the compiler made it and what for, and any other is an error, as is code that needs native code the program lacks.
- Compiling and loading are apart so that what was compiled can be kept: a program that keeps it compiles a function once, however often it is loaded and let go.
- Only a function that captured nothing is packed.
- A program whose row reaches loading is compiled as one code can be [loaded](linking.md) into; no other program pays for it.
- A program whose row reaches packing holds its own trees while it runs; no other does.
- TODO: a function that captured values is not packed, since what it captured would have to be checked against its definition where it arrives.
- TODO: a pack carries everything, so compiling one compiles again what the program already has.

## Values as bytes

- `system.exchange` gives a value as its [bytes](exchange.md) and the value of such bytes, for a program that keeps a value or sends one.
- They are [extern](externs.md) functions and no abilities, as hashing is: the bytes are a function of the value.
- Only plain data has bytes here: a value that holds a function, a cell, a handle, or an iterator is an error, since each is tied to the program that holds it.
- What is read back is of no known type; a program casts it to the type it expects.

## Cryptography

- `system.crypto` hashes bytes, and signs them with a key as an HMAC, by an algorithm a tag names: SHA-256, SHA-512, or SHA-1.
- They are [extern](externs.md) functions and no abilities: a hash is a function of what is hashed, so there is nothing to grant, bind, or record.
- Comparing two digests takes as long whatever they hold, so how long it took says nothing of where they differ.
- Random bytes are drawn through `system.random`, the one ability randomness has.
- SHA-1 is there for the protocols that still name it, and for nothing new.
- TODO: nothing encrypts, signs with a key pair, or hashes a password.
