# Modules

A module is a file of definitions. Every name a module uses comes from its own definitions, its imports, or its package's preludes.

## Modules

- A file is a module; a directory is a namespace and holds no definitions of its own.
- A definition is private to its module unless marked public, which makes it visible throughout its package.
- An import names each definition or module it brings in; no import brings in everything a module exports.
- An import of a package alone names its [library module](packages.md), which then qualifies paths as any imported module does.
- A module is not a value: it cannot be passed, stored, returned, or inspected.
- A module's name appears only in an import, in the library module's [list of public modules](packages.md), as the qualifier of a path to one of its definitions, and as an argument of a [comptime](comptime.md) call, which is handed which module it is and nothing of it.
- Every use of a module is therefore statically known, and naming a module demands none of its definitions.
- A path through a module names one definition, which is resolved where it is written: what a definition [reaches](build.md) is never more for the way its names were qualified.

## Paths

- A module has exactly one path: its package, then its file's location. No construct renames, moves, or aliases a module for another module.
- A path means the same thing inside its package and outside it; only its first segment differs.
- A path's first segment is the module's own package or one of that package's [dependencies](packages.md), under the local name the package gave it; nothing else can be named there, and nothing shadows it.
- Each later segment is resolved in the directory or module named to its left, from that module's definition table alone.
- A directory's members are the files and directories in it; a module's members are its definitions and the definitions it re-exports.
- A path segment names a file or a directory, never both, so every segment has exactly one source.
- A re-export applies to definitions only and is an [alias](symbols.md).
- Re-exports that lead back to the module they started from are an error.
- The [library module](packages.md) shares one scope with its package's top-level modules; a name in both is an error.
- Whether a path's last segment is a module or a definition is decided by resolution, not by how the path is written.

## Preludes

- A prelude is an ordinary module whose public definitions and aliases are in scope in every module of a package that lists it.
- A package lists its preludes in order; where two define the same name, the later one wins.
- A module's own definitions and imports shadow prelude names.
- A prelude from another package is a module that package's [library module](packages.md) lists.
- Preludes are set for a whole package and nowhere narrower; no module can add, remove, or reorder them.
- A package that lists none takes its workspace's list, and a workspace that lists none takes the standard library's.
- A platform or embedder supplies its vocabulary as a prelude, which its packages list.
- Prelude names are [aliases](symbols.md); what they bring into scope keeps its canonical symbol.
- Preludes do not vary by [target](targets.md): what a name resolves to never depends on where a program will run.
