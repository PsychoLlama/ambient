# Releases

A release is a publisher's statement that a version of a [package](packages.md) is a snapshot. A declaration says how a dependency may be updated; a lock says what is built.

## Versions

- A version is three numbers: a major, a minor, and a patch. A package's manifest states its version, so its snapshot covers it.
- A release that changes or removes the [interface](definitions.md) of anything public takes a new major, and one that only adds takes a new minor.
- Publishing compares the public interfaces of a release with those of the release before it, and refuses a version that says less than they changed.
- Only interfaces are compared: a release that changes what a function does, under the same interface, is the publisher's to number.
- Major zero is a major like any other.

## Releases

- A release is a record of a package ID, a version, and a snapshot, signed by a key of its publisher.
- A release never changes: a version of a package from a publisher is one snapshot for ever.
- A second record of one version with another snapshot is refused, and is evidence against the publisher.
- A publisher's releases of a package are a log that only grows. Withdrawing a release is a further record.
- A withdrawn release is never newly selected, and a lock that holds one builds as before.
- A release may carry further signed statements about its snapshot: who built it, and from what. Selection reads none of them.
- Publishing changes no file, so that a release is a revision of a repository is checked by hashing the revision.

## Publishers

- A publisher is a log of keys: its first record names a key, and each later one, signed by a key the log holds, adds a key or retires one.
- A publisher's ID is the hash of its first record, so its keys change and its ID does not.
- A reader never takes a log shorter than one it has read.
- A publisher that has lost every key is over. Its dependents adopt another publisher, as they would a fork.

## Selection

- A declaration of a released package gives its ID, its publisher, and a minimum version. The major of the minimum is the only upper bound.
- A workspace builds one snapshot of each package ID, publisher, and major: the highest minimum any declaration in it states.
- Selection therefore searches for nothing, and no set of declarations is unsatisfiable.
- Two majors of a package, or one package from two publishers, are distinct [package instances](packages.md), and one package may declare both under two local names.
- Updating a dependency raises the minimum in the manifest that declares it, so the minimum a package states is one it was built and tested with.
- What a [lock](#locks) holds stands while it satisfies every declaration: selection only raises it. A workspace updates a package it does not declare by raising the lock.
- A package ID that reaches a workspace from a second publisher is reported when it first does, since a package's [identities](definitions.md) are scoped by ID alone.

## Pinned dependencies

- A declaration may instead pin: it gives where a package is and exactly which state of it, and the lock holds that snapshot.
- A pinned dependency has no publisher and no version to select among. It changes only when its declaration does.
- Where a package was fetched from is a hint. A snapshot is the same from anywhere, since it is verified by its hash.
- A name that moves, such as a branch or a tag, is only ever a way to choose what to pin.

## Locks

- A workspace has one lock: each snapshot it builds that is not a member, by package ID, publisher, and version, with where it was fetched from.
- A build reads the lock and selects nothing. A declaration the lock does not satisfy is selected for, and the lock written.
- A lock is a list in order of package ID, publisher, and major, one entry to a snapshot, so that changes made apart merge.
- A lock is no part of any snapshot: whatever depends on a package selects for itself.
- A closure is the hash of a workspace's members' snapshots and the snapshots in its lock. It names everything a build reads.
- A closure is a hash of a list, never of what each snapshot depends on, so packages that depend on each other have one.
- A built program records its closure, so a definition's [hash](definitions.md) in a report leads to its source.

## Fetching

- A machine keeps snapshots by hash in one place, shared by every workspace. Nothing is installed into a workspace.
- Fetching runs no code of the package fetched.

## Vendoring

- Vendoring copies a package into a workspace as a [member](packages.md#workspaces). The workspace records the snapshot and publisher it was copied from; the copy itself is changed in nothing.
- A package that depends on another by a directory is not copied, since the copy is somewhere else.
- Every dependent in the workspace then builds the copy, as it would any member.
- A copy that is unchanged has the snapshot it was copied from and every definition's hash, so it builds what the snapshot did.
- A copy that is edited is a fork kept in the workspace. Its record says what to compare it with when the origin moves.

## Registries

- A registry is a tree of files: publishers' logs, their releases, snapshots by hash, and a table of names. Anything that serves files serves one.
- A registry is a convenience, not an authority: everything it serves is verified against a snapshot and a publisher's key, so one registry stands in for another.
- A name is a registry's label for a package ID and a publisher. It is read once, when a dependency is added, and the declaration holds what it led to.
- Reassigning a name affects no dependent.

## Approvals

- A workspace approves each package's [build grants](comptime.md) and [native code](externs.md), by package ID and publisher, those of packages it does not declare included.
- An approval is written in the workspace, where a review sees it.
- A snapshot that asks for more than was approved is not built until it is approved, so an update never widens what a dependency can reach.

## Unbuilt

- TODO: only vendoring is built, and what [packages](packages.md) says of members. There are no releases, publishers, registries, locks, or pinned dependencies, so only a package at a directory is vendored, and a build grant is taken as soon as it is declared.
