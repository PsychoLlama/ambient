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

- A package's public interface is rooted at its single library module, which re-exports definitions and modules.
- A re-exported module exposes its public definitions and whatever it re-exports in turn; a module no chain of re-exports reaches is internal.
- Public paths follow re-exports, not files, so a package can move a module without changing its interface.
- A package may define any number of named entries, each a function.
- A package declares its dependencies, its [preludes](modules.md), its [build grants](comptime.md), and its native bundles.
- A package is distributed as source.

## Workspaces

- Every package belongs to a workspace; a lone package is a workspace of one.
- A workspace has one [store](build.md), one [index](symbols.md), and one lock.

## Registries

- A registry maps names and package IDs to publishers and their versions.
- A registry is a convenience, not an authority: everything it serves is verified against a snapshot and a publisher's key.
