# Modules

Part of the [Ambient Language Reference](architecture.md).

Modules map 1:1 to files under `src/`; a directory is a namespace module
whose members are its children, so `src/net/http/client.ab` is the module
`pkg::net::http::client` with no `mod.rs`-style ceremony. Path roots:
`pkg` (package root), `self` (same directory), `super` (parent directory,
chainable), `core` (standard library — host bindings live under
`core::system`).

**Every item in a build has exactly one fully-qualified identity** — a
first-class `Fqn` (`crates/ambient-engine/src/fqn.rs`), not a string.

An `Fqn` is a `ModuleId` (a `Scope` + a scope-relative module path) plus an
ident path of one or more segments. There are two identity axes that
coexist:

- **Location** (the `Fqn`): the identity of every _resolved_ reference —
  same-module included — that the resolve pass canonicalizes: top-level
  items, type-associated members, and enum variants (the two-segment ident
  `[Enum, Variant]`). `Scope` is `Builtin` for the `core` standard library
  or `Workspace(pkg)` for a user package, so a user item renders
  `workspace::<pkg>::utils::helper` and a builtin renders
  `core::primitives::number::sqrt`. (`Scope::Library(hash)` is reserved for
  content-addressed dependencies.) This makes `a::b::c` unambiguous as
  data: `b` a submodule vs. `b` a type land on distinct `Fqn`s even though
  they render the same.
- **Content**: UUID-based method dispatch symbols (`<type-uuid>::method`)
  keep their perfect content identity and are _not_ folded into the `Fqn`.
  Content hashes never depend on the `Fqn`: finalization folds a recursive
  function's _bare_ short name into its group object, so the `Fqn` is a
  compile-time lookup key and post-hash label, never a hashing input.

Internal tables key off the `Fqn` struct (`Eq`/`Hash`) through a `NameKey`
(`Item(Fqn)` for a resolved location, `Bare(name)` for only two things:
true lexical locals and content-addressed `<uuid>::method` symbols — plus
registry-less checks/compiles, which never run resolve). `Display` exists
only for diagnostics and the on-disk store (`.ambient/store/names`), which
now records full `workspace::<pkg>::…` names.

The two access rules follow:

1. Anything reachable by its fully-qualified path works through `use`,
   and vice versa.
2. However a reference is spelled — bare same-module name, bare imported
   name, module-alias path, inline rooted path — it resolves to that one
   identity. The compiler front end canonicalizes every resolved reference
   before checking (`crates/ambient-engine/src/resolve.rs`), so the checker,
   the native-binding tables, the ability resolver, and the linker all key off
   the same `Fqn`; only true locals stay bare.

`use` takes a Rust-style use-tree:

```ambient
use pkg::utils::helper;                 // Item import: `helper` as a bare name
use pkg::utils::{a, deep::{b, c}};      // Brace groups, nested arbitrarily
use core::primitives::number::sqrt as root2;        // `as` renames the local binding
use {core::primitives::number, core::system::Stdio};    // Root-level groups
use self::utils;                        // Whole-module import: utils::helper(...)
use pkg::net::http;                     // Directory namespaces import too:
use http::client::get;                  //   ...and a module alias can root
                                        //   another use (any order; resolved
                                        //   by fixed point)
use pkg::shapes::Shape;                 // Enum import: type, constructors, patterns
```

Braces are pure grouping — the tree flattens during lowering, so
`use a::b::{c, d};` is exactly `use a::b::c; use a::b::d;`. A flattened
path names an entity by its final segment, and the resolver binds every
namespace meaning that exists: a submodule, a value, a type, an ability.
Modules, values, types, and abilities occupy separate namespaces resolved
by syntactic position, so a name that is both a submodule and an exported
item binds both and the use site disambiguates (`c(...)` is the value,
`c::foo` the module).

`use` is also a statement: a block-scoped import binds from its statement
to the end of the enclosing block, types as nothing, and compiles to
nothing.

```ambient
pub fn hyp(a: Number, b: Number): Number {
  use core::primitives::number::sqrt;
  sqrt(a * a + b * b)
}
```

Inline fully-qualified paths need no import anywhere a name can appear:
expressions (`pkg::utils::helper(1)`), type positions
(`pkg::shapes::Money`, `core::time::Duration`), effect rows
(`with core::system::Stdio`, `with pkg::effects::Counter`), performs, handler
arms, and sandbox clauses. Local bindings shadow module-level names,
which shadow imports, which shadow the prelude.

`pub` gates every export: functions, consts, types, enums, abilities, and
traits are module-private unless declared `pub`. A failed import — missing
module, missing symbol, or private symbol — is a compile error at the `use`
item, never a silent no-op. Importing an enum brings its variant
constructors and patterns into scope wholesale, as if declared locally;
a single variant cannot be imported on its own, and a qualified _type_
reference alone (`pkg::shapes::Shape` in a signature) does not bring
constructors into scope — import the enum where you construct or match
it. `pub use` re-exports items (and whole modules), and imports through a
re-export resolve (and link) to the module that defines the symbol.
Re-export paths must be rooted (`pkg`/`core`/`self`/`super`),
not alias-relative, so downstream modules can resolve them without this
module's scope.

Core modules (`core::collections::list`, `core::primitives::number`,
`core::primitives::string`) are ordinary Ambient modules — compiled,
content-addressed, and stored exactly like user code (see
`crates/ambient-engine/src/core_lib/`). Their low-level operations are
`extern fn` declarations (`core::primitives::number::sqrt`,
`core::collections::list::length`, `core::primitives::string::concat`, ...):
signatures owned in the `.ab` source, implementations bound by the host
under stable UUIDs (see [extern fns in core-library.md](core-library.md#native-functions-extern-fn)).
An extern fn is an ordinary item of its module — importable, aliasable,
first-class, reachable through `use core::primitives::number;` +
`number::sqrt(x)`. `core` is a keyword
and a user module may not take that name (the build rejects `src/core.ab`),
so the one reserved namespace — which now also houses the host bindings at
`core::system` — can never be shadowed. `platform` is an ordinary
identifier again: `src/platform.ab` is a perfectly legal user module.

The `core::` hierarchy is defined by the `core_lib/` source tree itself, not
by a hand-maintained list: `register_core_modules` walks the embedded tree and
maps every `.ab` file to a module path through the _same_ file↔module mapping
(`ModulePath::from_relative_file_path`) that user packages use, and each
directory's `main.ab` (a _directory module_) groups its siblings with `pub use
self::…`. A directory module anchors `self`/`super` at its own path rather than
its parent, so `core_lib/collections/main.ab` re-exports its neighbours as
`core::collections::…`. Native bindings are the one Rust-side coupling: the
engine's registry (`natives/core/`) keys off the resolved module path, so it
must name the same paths the tree registers — the build fails loudly on any
drift (`ModuleRegistry::verify_native_contract`).

Module files are snake_case, types PascalCase: `core::primitives::string`
is the companion module of the `String` type, `core::collections::list` of
`List<T>`, `core::option` of `Option<T>`. Related modules group under
namespace parents — `core::collections` (`list`, `map`, `set`),
`core::primitives` (`string`, `number`, `bool`, `binary`) — while top-level
namespaces like `time`, `convert`, and `reflect` name no single type. Types,
values, and modules occupy separate namespaces resolved by syntactic
position, so the type `List` and the module `core::collections::list`
coexist without ambiguity.

Known gaps (deliberate, minor): qualified references to _generic_ type
aliases and generic `unique` types are unresolved (parameter substitution
is checker work); ability names inside function _type_ annotations
(`(T) -> U with E`) accept only bare or `core::system::` spellings.

A local binding shadows a module alias: after `let utils = ...;`,
`utils.method()` is a trait-method call on the value.
