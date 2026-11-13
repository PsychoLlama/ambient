# Symbols

A symbol is the one canonical name of a definition within a workspace. Every other way of naming a definition is an alias, resolved away before anything is checked.

## Identity

- A symbol is a [package instance](packages.md), a module path, a namespace, and a name path.
- The namespaces are type, value, ability, and key; a module may define the same name once in each.
- Types live in the type namespace; functions, externs, and targets in the value namespace; abilities and row aliases in the ability namespace; [keys](methods.md) in the key namespace.
- A nested named function extends its encloser's name path.
- Lambdas, parameters, and local binders are addressed by their owning symbol and their position in its tree, never by source position.
- A symbol has exactly one definition site; a second is an error.
- A symbol maps to exactly one hash; a hash may belong to many symbols.

## Aliases

- Imports, re-exports, and local names bound to a definition are aliases.
- An alias resolves to exactly one symbol; chains of aliases collapse to their end.
- A [module](modules.md) is aliased the same way, and its alias resolves to exactly one module.
- After resolution a tree contains no aliases: every reference is a symbol.
- Checking, diagnostics, and tools see only symbols, so behaviour never depends on the path by which a name was reached.
- An alias has no hash and no interface.
- A definition reachable by symbol is reachable through any visible alias, and the reverse.

## Resolution

- Resolving a name needs the definition tables of the current module, the modules it imports, and its package's [preludes](modules.md), never a type or a body.
- A module's definition table is readable from its file alone, except for [generated modules](comptime.md).
- Every name in a module is attributable by reading that module: imports list what they bring in, and preludes are the only implicit scope.
- Record fields and tags are not symbols, because resolving them would need types.

## Index

- The index maps each symbol to its definition site, interface hash, and hash, and each reference site to the symbol it resolves to.
- The index is derived from sources and kept current lazily, module by module, keyed by module content.
- Go-to-definition, find-references, rename, and move are index queries.
- A query costs the same from a one-shot command as from a running server; see [build](build.md).

## Migrations

- Renaming or moving a symbol rewrites its reference sites and nothing else, and needs no types.
- A migration is an [edit](trees.md) addressed by symbol.
- A package release may carry migrations from old public names to new ones, which dependents apply mechanically.
