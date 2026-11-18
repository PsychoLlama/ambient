# Releases

A release is a publisher's statement that a version of a [package](packages.md) is a snapshot. A declaration says how a dependency may be updated; a lock says what is built.

## Versions

- A version is a semantic version: a major, a minor, and a patch, and what may mark it a pre-release. A package's manifest states its version, so its snapshot covers it.
- A version's line is its major; under major zero its minor as well; under minor zero too, all three. Versions of one line are compatible.
- A release that changes or removes the [interface](definitions.md) of anything public takes a new line. Above major zero, one that only adds takes a new minor.
- Publishing compares the public interfaces of a release with those of the release before it, and refuses a version that says less than they changed.
- Only interfaces are compared: a release that changes what a function does, under the same interface, is the publisher's to number.

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

- A declaration of a released package gives its ID, its publisher, and a minimum version. The [line](#versions) of the minimum is the only upper bound.
- A workspace builds one snapshot of each package ID, publisher, and line: the highest minimum any declaration in it states.
- Selection therefore searches for nothing, and no set of declarations is unsatisfiable.
- Two lines of a package, or one package from two publishers, are distinct [package instances](packages.md), and one package may declare both under two local names.
- Updating a dependency raises the minimum in the manifest that declares it, so the minimum a package states is one it was built and tested with.
- What a [lock](#locks) holds stands while it satisfies every declaration: selection only raises it. A workspace updates a package it does not declare by raising the lock.
- A package ID that reaches a workspace from a second publisher is reported when it first does, since a package's [identities](definitions.md) are scoped by ID alone.

## Git dependencies

- A declaration may instead name a git repository, a directory in it, and a branch, a tag, or a commit of it. With none of the three it means the repository's default branch.
- A declaration is written by a person and says what an update may move to. The lock is written by the tool and says what is built: the commit the declaration led to, and the snapshot there.
- A branch or a tag is read once, when the lock holds nothing for the declaration. No command asks a repository whether it has moved, except an update.
- A git dependency has no publisher and no version to select among: nothing is looked for among a repository's tags.
- Where a package was fetched from is a hint. A snapshot is the same from anywhere, since it is verified by its hash.

## Locks

- A workspace has one lock: each snapshot it builds that is not a member, by package ID, publisher, and version, with where it was fetched from.
- A build reads the lock and selects nothing. A declaration the lock does not satisfy is selected for, and the lock written.
- Every command settles the lock against the declarations of every member: an entry none of them leads to is dropped. Editing a manifest therefore never leaves a workspace that does not build.
- A lock is a list in a fixed order, one entry to each snapshot a declaration leads to, so that changes made apart merge.
- A lock is no part of any snapshot: whatever depends on a package selects for itself.
- A closure is the hash of the snapshots in a workspace's lock. It names everything a build reads from outside its workspace.
- A closure is a hash of a list, never of what each snapshot depends on, so packages that depend on each other have one.
- A built program records its closure, so a definition of a locked snapshot leads to its source.

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

- TODO: git dependencies, the lock, fetching, and vendoring are built, and what [packages](packages.md) says of members. There are no releases, publishers, or registries, so nothing is selected, and no closure is recorded.
- TODO: a build grant is taken as soon as it is declared, by a fetched package as by a member.
