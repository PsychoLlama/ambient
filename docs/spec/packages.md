# Packages

A package is a tree of modules whose identity does not depend on any name. Names are for discovery and local reference; identity is a package ID, a publisher, and a snapshot.

## Identity

- A snapshot is a hash of a package's own contents: its manifest, its sources, the other files its manifest lists, and the digests of its [native bundles](externs.md).
- A snapshot covers no other package: a manifest names a dependency by its ID or by where it is, never by snapshot. Packages that depend on each other therefore each have one.
- A snapshot identifies exactly one state of a package and never changes meaning.
- A snapshot is the same wherever the package is: in a checkout, in an archive, or copied into another workspace.
- A snapshot is how a package is distributed. A member of a workspace is built from its files as they are, and what those are is its repository's to say.
- A package ID is a generated identifier, fixed when the package is created and shared by every snapshot of it, forks included.
- A [publisher](releases.md#publishers) is a key-holding identity that releases snapshots; trust in updates attaches to a publisher, not to a package ID or a name.
- A [version](releases.md#versions) is a publisher's ordering of its own snapshots of one package ID; versions from different publishers are unrelated.
- A name identifies nothing: a package's own is for messages, and a dependent's is its own choice.

## Dependencies

- A dependency declaration binds a local name to a package: the one at a directory, the one in a [git repository](releases.md#git-dependencies), or one named by its ID, which is a [member](#workspaces) or a [release](releases.md#selection).
- A package's ID is said in one place, its own manifest. A declaration that says where a package is states no ID: the package there has one, and a second statement of it could only be wrong.
- The local name is an identifier chosen by the depending package, and is the only name its modules use for the dependency.
- A module can name its own package and its package's declared dependencies, and nothing else; transitive dependencies are out of reach.
- Packages may depend on each other, in a workspace and released.
- Adopting a fork changes the publisher in the declaration and changes no import.
- A build takes only what its entry [reaches](build.md), so a dependency costs a program only what the program uses of it. No declaration is optional, and none is switched on.

## Package instances

- A package instance is a workspace member or a [locked](releases.md#locks) snapshot; it is the package component of a [symbol](symbols.md).
- Two snapshots of one package ID may coexist in a workspace as distinct package instances.
- Coexisting snapshots share every definition that is the same in both, since a definition is kept by its [hash](definitions.md).
- Coexisting snapshots share [tags](definitions.md), so an ability whose signature is unchanged between them is one ability.

## Contents

- A package has a single library module, whose definitions and re-exports are reached through the package itself.
- The library module lists the package's public modules by [path](modules.md); a module it does not list is internal.
- A public module exposes its public definitions to dependents; nothing else leaves the package.
- A public module is reached by the same path its own package uses, so moving one is a change of interface, made as an [edit](trees.md) across dependents.
- A package may define any number of named entries, each a function.
- A package declares its version, its dependencies, its [preludes](modules.md), its [build grants](comptime.md), its [build steps](tools.md#builds), its [lints](tools.md#commands), its native bundles, and the files beside its sources that are part of it.
- A package is distributed as source.

## Layout

- A package is a directory: a manifest, and its modules as source files under one directory of sources.
- A source file is the [module](modules.md) its path under the sources names, and the file named `lib` at their top is the library module.
- The manifest says the package's name, its ID, and its version, and lists its dependencies, its preludes, its [build grants](comptime.md), each the path of an ability, its [build steps](tools.md#builds), each the paths of a target and of a tool, its [native bundles](externs.md#distribution), each a platform and a digest, and its other files.
- A dependency is listed by its local name, with where the package is, a directory or a git repository, or else its ID and what [selects](releases.md#selection) it.
- A dependency at a directory is on the package there. It states no ID and no version: the package there is a member, as it is.
- A file of the package is one under its sources or one its manifest lists. Nothing else in its directory is part of it, and no [comptime](comptime.md) call reads anything else.
- The manifest may list a file by the digest of its content and somewhere to fetch it from, in place of keeping it. A snapshot covers the digest, so such a file is as fixed as one that is kept.
- Where a file is fetched from is a hint, as a [snapshot's](releases.md#git-dependencies) is: the digest is what is held to.
- A member's file listed by digest may also be given a place in its directory. What is there is used as it is, which is how such a file is worked on; with nothing there it is fetched.
- No package's ID is the standard library's, or the [`system`](targets.md) package's.
- Every package reaches the standard library under the name `ambient`, and the `system` package under its own, without declaring them.

## Running

- The command line runs a function of a package as a program under the `system` package's command-line [target](targets.md), and exits with what it gives if that is an integer.
- It may instead be run under a target the package names that extends that one. That is how a program binds a package's [native handler](externs.md), which only a target can.
- A [fault](values.md) that ends the program is reported as its error: the kind, the message, and the definitions its trace names.
- Checking a package checks every definition of it, and of the packages it depends on what those reach, and reports each that is wrong. Each is reported by file, line, and column.
- TODO: checking a package checks every definition of each package it depends on, reached or not.
- A definition of a dependency that no member reaches is therefore never read, and can be wrong without anyone who depends on its package being told.
- Built definitions are kept in one [store](build.md) for every project on the machine, since a definition is kept by its content.

## Building

- Building makes an executable of a function of a package: one file that runs the program on a machine that has neither the package nor the compiler.
- The executable is the program as compiled for the machine that built it, and what loads and runs it; nothing is compiled when it starts.
- It runs as the command line runs the function: under the same [target](targets.md), started with every argument it is given, exiting with what the function gives.
- A [fault](values.md) is reported as it is when run from the package: the executable carries the name of each definition it holds and the place of each expression that can fault, as they were when it was built.
- An executable never reads a [store](build.md) and never writes one.
- A build may instead be of a function under a target the package names, which gives what that target's [tool](tools.md#builds) does: a WebAssembly module and whatever its [host](targets.md#webassembly-hosts) loads it with.
- A target that extends the command-line target is built as that one is: an executable.
- What loads the program holds no compiler: an executable carries only what running needs, and can [load](exchange.md) compiled code as a program run from its package can.
- A program that packs or compiles code needs the compiler as it runs, so what loads that one is the compiler's own executable.
- An executable whose program [packs](targets.md#code) code carries the trees of what it could pack; one that does not carries none.
- What loads the program is [linked](linking.md#native-code) with the [native bundles](externs.md#native-bundles) the program reaches, so an executable is one file whatever native code its packages bring.
- TODO: an executable is only for the machine and system that built it; building for another needs what loads the program built for that one.

## Workspaces

- Every package belongs to a workspace; a lone package is a workspace of one.
- A workspace is a directory whose manifest lists its members, each the directory of a package. The workspace's own directory may be one of them.
- A package's workspace is the nearest directory above it whose manifest lists it, and a package at a directory a member depends on is a member too.
- A member stands in for every dependency in its workspace on its package ID and [line](releases.md#versions) of versions, whatever publisher the declaration names.
- Two members of one package ID and line are an error, so what stands in is never a choice.
- Two members of one name are an error: a name identifies nothing, and two packages that share one are taken for a mistake.
- A member takes nothing from its workspace's manifest: a package means the same in any workspace.
- A command that checks or tests, run at a workspace's directory, covers every member.
- A workspace has one [store](build.md), one [index](symbols.md), and one [lock](releases.md#locks).

## Beginning

- A new package is made in a directory and named for it: a manifest with a newly generated ID, and one module that runs and has a [test](#testing), so the first change is made to something that works.
- A source file that is there already is left as it is.
- A package is not made over one that is there.

## Testing

- Tests are built in: the standard library has what a test is written with, and the command line has a harness that runs them.
- A module offers its tests through one function the harness knows by name, whose [row](abilities.md) is the library's spec ability. Importing a module offers nothing.
- A spec is a description and a function that takes nothing; naming one and grouping several are library functions that perform the ability.
- A group may [provide](handlers.md) around the specs inside it, which is how they share what they are set up with.
- The harness gathers a module's specs by running that function under a handler that collects them. Gathering is pure, so it is a [memoized run](memoization.md).
- Each spec is run as a program of its own under the standard library's test [target](targets.md), as a memoized run.
- Running every test therefore runs only those a change reaches: a spec's hash covers all it refers to, and one whose hash and observations are as before is not run again.
- A harness is a handler of the spec ability, so another may list, choose among, or time the same tests.
- The harness finds the modules that offer tests in the package's definition tables; no file is named in a special way.
- The test target binds one ability, which is given a failure as a value: what did not hold, and whatever more it holds, as what was found and what was expected. Every assertion is a library function over it, and nothing written is one.
- A check that does not hold fails its test and the test goes on, so one run reports every check that does not hold; a [fault](values.md) fails it and ends it.
- A test reaches no world: one whose function needs an ability provides it, which is how a test says what the world holds.
- A test of real effects is made repeatable by library code over [recording](handlers.md): its first run records what the world answered, and later runs are answered from that.
- A recording is source, kept beside its test and read as any file is; the [store](build.md) holds only results, and whoever points a build at a store trusts it.
- A run may be of only the specs whose module path and description hold a given text.
- TODO: little of this is built. A test is a function that takes nothing and whose name begins with `test_`. A test that passed is not run again while its hash is the same; one that failed always is.
- TODO: a spec's function has the row the library gives it, so a test that needs more provides it inside; whether a spec may be written over any row is undecided.
- TODO: a test that never ends is never stopped.
