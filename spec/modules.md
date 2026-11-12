# Modules

A module is a file of definitions. Every name a module uses comes from its own definitions, its imports, or its package's preludes.

## Modules

- A file is a module; a directory is a namespace and holds no definitions of its own.
- A path segment names a file or a directory, never both.
- A definition is private to its module unless marked public, which makes it visible throughout its package.
- An import names each definition or module it brings in; no import brings in everything a module exports.
- A module is not a value: it cannot be passed, stored, returned, or inspected.
- A module's name appears only in an import, in a re-export, and as the qualifier of a path to one of its definitions.
- Every use of a module is therefore statically known, and naming a module demands none of its definitions.
- A module path starts at the module's own package or at one of that package's [dependencies](packages.md), under the local name the package gave it.

## Preludes

- A prelude is an ordinary module whose public definitions and aliases are in scope in every module of a package that lists it.
- A package lists its preludes in order; where two define the same name, the later one wins.
- A module's own definitions and imports shadow prelude names.
- Preludes are set for a whole package and nowhere narrower; no module can add, remove, or reorder them.
- A package that lists none takes its workspace's list, and a workspace that lists none takes the standard library's.
- A platform or embedder supplies its vocabulary as a prelude, which its packages list.
- Prelude names are [aliases](symbols.md); what they bring into scope keeps its canonical symbol.
- Preludes do not vary by [target](targets.md): what a name resolves to never depends on where a program will run.
