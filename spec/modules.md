# Modules and Packages

A module is a file of definitions, a package is a named tree of modules, and a workspace is a set of packages built together.

## Modules

- A file is a module; a directory is a namespace and holds no definitions of its own.
- A path segment names a file or a directory, never both.
- A definition is private to its module unless marked public, which makes it visible throughout its package.
- An import names each definition or module it brings in.

## Packages

- A package is named; its name is the root of its module paths and the prefix of its [tags](definitions.md).
- A package's public interface is the set of definitions aliased from its single library module; everything else is internal.
- A package may define any number of named entries, each a function.
- A package declares its dependencies, its [build grants](comptime.md), and its [native bundles](externs.md).
- A package is distributed as source and identified by a hash of its contents; a published version is immutable.

## Workspaces

- Every package belongs to a workspace; a lone package is a workspace of one.
- A workspace has one [store](build.md) and one [index](symbols.md).
