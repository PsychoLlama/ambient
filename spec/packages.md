# Packages

A package is a tree of modules whose identity does not depend on any name. Names are for discovery and local reference; identity is a package ID, a publisher, and a snapshot.

## Identity

- A snapshot is a hash of a package's entire contents: sources, manifest, and the digests of its [native bundles](externs.md).
- A snapshot identifies exactly one state of a package and never changes meaning.
- A package ID is a generated identifier, fixed when the package is created and shared by every snapshot of it, forks included.
- A publisher is a key-holding identity that releases snapshots; trust in updates attaches to a publisher, not to a package ID or a name.
- A version is a publisher's ordering of its own snapshots of one package ID; versions from different publishers are unrelated.
- A name is a registry's label for discovery; it identifies nothing, and reassigning it affects no dependent.

## Dependencies

- A dependency declaration binds a local name to a package ID, a publisher, and a version constraint.
- The local name is an identifier chosen by the depending package, and is the only name its modules use for the dependency.
- A module can name its own package and its package's declared dependencies, and nothing else; transitive dependencies are out of reach.
- A lock records the snapshot, package ID, and publisher of every dependency.
- An update is a newer snapshot of the same package ID from the same publisher.
- Adopting a fork changes the publisher in the declaration and changes no import.
- A dependency without a registry name is still a complete dependency.

## Package instances

- A package instance is a workspace member or a locked snapshot; it is the package component of a [symbol](symbols.md).
- Two snapshots of one package ID may coexist in a workspace as distinct package instances.
- Coexisting snapshots share [tags](definitions.md), so an ability whose signature is unchanged between them is one ability.

## Contents

- A package has a single library module, whose definitions and re-exports are reached through the package itself.
- The library module lists the package's public modules by [path](modules.md); a module it does not list is internal.
- A public module exposes its public definitions to dependents; nothing else leaves the package.
- A public module is reached by the same path its own package uses, so moving one is a change of interface, made as an [edit](trees.md) across dependents.
- A package may define any number of named entries, each a function.
- A package declares its dependencies, its [preludes](modules.md), its [build grants](comptime.md), and its native bundles.
- A package is distributed as source.

## Layout

- A package is a directory: a manifest, and its modules as source files under one directory of sources.
- A source file is the [module](modules.md) its path under the sources names, and the file named `lib` at their top is the library module.
- The manifest says the package's name and its ID, and lists its dependencies, each by the local name and where the package is, and its preludes.
- No package's ID is the standard library's.
- Every package reaches the standard library under the name `ambient` without declaring it.
- TODO: a dependency is only a path to a directory; there are no publishers, versions, locks, or registries yet, and nothing checks that an ID was generated.

## Running

- The command line runs a function of a package as a program under the standard library's command-line [target](targets.md), and exits with what it gives if that is an integer.
- A [fault](values.md) that ends the program is reported as its error: the kind, the message, and the definitions its trace names.
- Checking a package checks every definition of it and of the packages it depends on, and reports each that is wrong. Each is reported by file, line, and column.
- Built definitions are kept in one [store](build.md) for every project on the machine, since a definition is kept by its content.

## Workspaces

- Every package belongs to a workspace; a lone package is a workspace of one.
- A workspace has one [store](build.md), one [index](symbols.md), and one lock.

## Registries

- A registry maps names and package IDs to publishers and their versions.
- A registry is a convenience, not an authority: everything it serves is verified against a snapshot and a publisher's key.

## Beginning

- A new package is made in a directory and named for it: a manifest with a newly generated ID, and one module that runs and has a [test](#testing), so the first change is made to something that works.
- A source file that is there already is left as it is.
- A package is not made over one that is there.

## Testing

- A test is a function that takes nothing and whose name begins with `test_`, in any module of the package.
- Each test is run as a program of its own under the standard library's test [target](targets.md), which binds one [ability](abilities.md): a check, given whether something holds and what it is.
- A check that does not hold fails its test and the test goes on, so one run reports every check that does not hold; a [fault](values.md) fails it and ends it.
- A test reaches no world: one whose function needs an ability [provides](handlers.md) it, which is how a test says what the world holds.
- A test that names an ability its target does not bind is not a program, and fails for that.
- A run may be of only the tests whose module path and name hold a given text.
- TODO: a test that never ends is never stopped.
