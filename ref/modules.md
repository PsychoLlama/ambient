# Modules

Part of the [Ambient Language Reference](architecture.md).

Modules map 1:1 to files under `src/`; a directory is a namespace module
whose members are its children, so `src/net/http/client.ab` is the module
`pkg::net::http::client` with no `mod.rs`-style ceremony. Path roots:
`pkg` (package root), `self` (same directory), `super` (parent directory,
chainable), `core` (standard library — host bindings live under
`core::system`), and a bare leading `::` (a sibling workspace package —
see [Workspaces](#workspaces)).

The package root `main.ab` **is** the package: it behaves as the package's
directory module, so its items are `pkg::<item>` (and, cross-package,
`::<package>::<item>`) — there is no addressable module named `main`.

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
use core::convert::to_string as show;               // `as` renames the local binding
use {core::primitives::number, core::system::Stdio};    // Root-level groups
use self::utils;                        // Whole-module import: utils::helper(...)
use pkg::net::http;                     // Directory namespaces import too:
use http::client::get;                  //   ...and a module alias can root
                                        //   another use (any order; resolved
                                        //   by fixed point)
use pkg::shapes::Shape;                 // Enum import: type, constructors, patterns
use pkg::fx::Random::seed;              // Ability-method import: `seed!(…)` performs bare
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
pub fn describe(n: Number): String {
  use core::convert::to_string;
  "n = " + to_string(n)
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
it.

Ability **methods** are the one member kind importable on their own:
`use pkg::fx::Random::seed;` lets perform sites drop the prefix
(`seed!(…)` for `Random::seed!(…)`). The binding is calls-only and
occupies its own namespace (a perform is syntactically distinct —
`name!(…)` — so a method import never collides with a same-named function
or type); handler arms, `with` rows, and `Handler<A>` types still name
the ability. Method imports alias (`as`), re-export (`pub use`), and
block-scope like any other leaf; visibility is the ability's (`pub
ability` exposes its methods, exactly as `pub enum` exposes variants).
See [Using Abilities](abilities.md#using-abilities). `pub use` re-exports items (and whole modules), and imports through a
re-export resolve (and link) to the module that defines the symbol.
Re-export paths must be rooted (`pkg`/`core`/`self`/`super`/`::package`),
not alias-relative, so downstream modules can resolve them without this
module's scope.

The module dependency graph is a hard DAG: an import cycle **between**
modules is a compile error (Go's rule). If `pkg::a` references anything in
`pkg::b` and `pkg::b` references anything back in `pkg::a`, the build
rejects it with `import cycle: demo::a -> demo::b -> demo::a` — a named
path over the modules' mounted keys (the leading segment names the owning
package, so a cross-package cycle reads unambiguously), rendered
deterministically (rotated to start at the lexically-least module)
regardless of file order — rather than an arbitrary compile order that
surfaces as confusing link failures. Recursion stays **within** a module:
a function calling itself or a sibling, or a `use self::…` of the module's
own items, is a same-module reference and never a dependency edge, so it is
always allowed. The decision lives once in the engine
(`crates/ambient-engine/src/module_cycles.rs`), shared by `build_package`
and the analysis pipeline, so `ambient run`, `ambient check`, and the LSP
report the identical cycle at every module that participates in it. Core
and platform module groups are authored cycle-free and compiled on their
own ordering; cycle detection spans every _user_ module — cross-package
`::pkg` edges make workspace-wide cycles possible, and they are just as
fatal — while no `core`/`platform` module can import user code, so a user
cycle can never route through them.

## Workspaces

A workspace groups several first-party packages under one root
`ambient.toml` (`crates/ambient-engine/src/workspace.rs`):

```toml
[workspace]
members = ["app", "greeting_lib", "tools/deploy"]
```

The root manifest is _virtual_ — `[workspace]` and `[package]` are
mutually exclusive — and each member is an ordinary package listed by
directory. Libraries (content-addressed third-party dependencies,
`Scope::Library`) are explicitly out of scope: a workspace is first-party
code only. Member package names must be unique across the workspace, a
member may not take the reserved name `core`, workspaces do not nest, and
a package sitting under a workspace root that does not list it is a hard
error (never a silent standalone build with a split cache). Discovery is
one upward walk (`Workspace::discover`), shared by the build, `check`, the
LSP, and the store commands, so every frontend agrees on which packages
participate.

**Cross-package references** spell with a bare leading `::` — the first
segment names the sibling package, the rest walks its module tree exactly
like `pkg::…` walks your own:

```ambient
use ::greeting_lib::greet;            // an item of the package's root main.ab
use ::greeting_lib::casing::shout;    // a submodule item
use ::greeting_lib::casing;           // a whole-module alias
pub use ::greeting_lib::greet;        // re-exports compose across packages
```

The same paths work inline in expression, type, and pattern position
(`::greeting_lib::greet()`, `fn f(s: ::greeting_lib::Shape)`,
`::greeting_lib::Shape::Circle(r) =>`); the AST carries the workspace
root as a leading empty path segment, and resolution is byte-for-byte
the `use ::` rule. Ordinary visibility applies — only `pub` items cross
the boundary — and explicit references are the only channel: there is no
implicit sharing, and the `[workspace]` manifest carries no dependency
list (the dependency graph is whatever `::…` references say it is).

**Identity and mounting.** Every package's modules register under a
leading package-name segment (the package's _mount_): `foo`'s
`src/utils.ab` is the registry module `foo::utils`, and its root `main.ab`
collapses to the mount itself as a directory module — the same
`<dir>/main.ab` rule the module system already has. `ModuleRegistry`
strips a mount into `Scope::Workspace(<name>)` exactly like a leading
`core` strips into `Builtin`, so mounted `ModulePath`s and `ModuleId`s
convert totally in both directions and several packages share one
registry with no key collisions and no second naming convention. A
standalone package mounts alone, so the two layouts never diverge. The
mount is also a hard resolution boundary: `pkg::` anchors at it and
`super` may never step above it into a sibling package.

**One build, lazy targets.** A build loads and registers _every_ member
(so `::pkg` references resolve and trait coherence sees all impls), but
only compiles its **targets**: `ambient build`/`check` at the workspace
root covers every member; `ambient build <member>` (or `--package NAME`)
checks and compiles only that package plus the sibling modules its
compile order actually reaches. `ambient run` needs a target package
(implied inside a member directory; `--package` at a multi-member root)
and deploys the entry under its canonical `workspace::<pkg>::<entry>`
name, so a dependency's same-named `run` can never be picked. One caveat
of the everything-registers shape: every member must _parse_ for any
member to build (type errors stay lazy; syntax errors are workspace-wide).

**One store.** The `.ambient/store` lives at the workspace root and is
shared by all members — content-addressed objects dedupe across packages,
and one member's build warms the core cache for every other. Snapshot
pointers are per package (`snapshots/<name>`): a member build repoints
only its own snapshot, a workspace-root build points every member at the
same manifest, gc roots the union, and the incremental cache reads all
pointers' manifests. See `crates/ambient-engine/src/disk_store/`.

The `examples/` directory is itself a workspace; `examples/workspace_deps`
imports the library member `examples/greeting` and is the living
demonstration.

## Lazy compilation (module-level reachability)

`ambient run` compiles only the package modules **reachable** from its entry
point; a module the entry can't reach is never checked or compiled. Every
other frontend stays whole-package: `ambient check` and the LSP diagnose the
entire package, `ambient build` emits a whole-package artifact, and
`ambient dev` builds every module (its deploy diff needs every module's
bindings, and it is a snapshot writer — see below). The reachability decision
lives in `crates/ambient-engine/src/build/reachability.rs`, gated by
`BuildOptions::entry` (only `ambient run` sets it, via `build_reachable`).

**The reachability rule.** Starting from the module(s) declaring the entry
function, a module is reachable if it is in the transitive closure of two
edge kinds:

1. **Resolve-pass dependencies** (the forward graph: `use`, inline qualified
   paths, enum-variant construction, foreign consts, ability performs, and
   ability default-implementation bodies — the resolve pass walks those like
   any function body). This alone covers abilities end to end: you cannot
   perform an ability without a dependency on its module, and that module's
   default-impl body carries its own dependencies.
2. **Trait/inherent-impl dispatch** (the coherence channel). A `x.method()`,
   `a + b` on a nominal type, or `Type::assoc(..)` links a content-addressed
   `<type-uuid>::<trait-uuid>::method` symbol defined in whichever module
   wrote the `impl` — and there is **no orphan rule**, so that module need not
   be imported by the dispatcher (an `impl Show for Widget` can live in a
   third module). We cannot read the checker's dispatch symbols without
   type-checking (which the policy forbids for unreachable modules), so we
   recover the edge structurally: to dispatch an impl for a type `T`,
   reachable code must hold a `T` value, so `T`'s defining module is always
   reachable — therefore every impl-defining module is made reachable **from
   its impl's target-type module** (a reverse edge). When the target type is a
   builtin/prelude type (or the impl is a blanket/param impl), reachable code
   can hold the value with no package dependency, so we cannot prove the impl
   unreachable and include the module **unconditionally**.

This is a sound over-approximation: spurious inclusion only costs compile
time, never correctness. It is module-grain, not item/FQN-grain — a reachable
module compiles whole (the checker's intra-module monomorphic coupling blocks
finer laziness). A reached module's objects are **byte-identical** to a
whole-package build's, because the lazy pass filters the whole-package compile
order rather than recomputing it, so each reached module compiles in the same
relative order and against the same accumulated linking state.

**Diagnostics policy.** `ambient run` does **not** report diagnostics in
unreachable modules — a type error, import error, or coherence conflict in a
module the entry can't reach never fails a run (that module is never checked).
`ambient check` and the LSP are whole-package and report them. The rule: if a
program's behavior depends on a module, that module is reachable and is
checked; a module nothing reachable observes is the compiler's business only
when you ask for a full check.

**Snapshot semantics.** A lazy build **reads** the package-local store (warm
cache hits still apply to reached modules) but writes **no** snapshot. This is
the simplest sound choice for a partial build: a lazy build never records the
unreached modules, so persisting its manifest would either strand ghost
records or, if the manifest were marked partial, mislead `ambient store diff`
(which computes removals) and the store gc (whose roots are a snapshot's
referenced objects). The snapshot writers are the whole-package commands
(`ambient build`, `ambient dev`); a lazy `ambient run` fully exploits a
snapshot they left but can never corrupt one. The trade-off — a run-only loop
doesn't warm the store from `run` itself — is acceptable and reversible: a
future step could carry a prior snapshot's unreached records forward.

Core modules (`core::collections::list`, `core::primitives::number`,
`core::primitives::string`) are ordinary Ambient modules — compiled,
content-addressed, and stored exactly like user code (see
`crates/ambient-engine/src/core_lib/`). Their low-level operations are
**module-private** `extern fn` declarations (`sqrt`, `length`, `concat`,
...): signatures owned in the `.ab` source, implementations bound by the
host under stable UUIDs (see [extern fns in core-library.md](core-library.md#native-functions-extern-fn)).
The public surface is the inherent `impl` on each type — receiver methods
(`x.sqrt()`, `list.map(f)`) and associated functions (`List::range`,
`Binary::from`, `String::join`), never the raw extern. The core free
functions that _are_ public (`core::convert::to_string`,
`core::option::flatten`, ...) stay ordinary items of their module —
importable, aliasable, first-class, reachable through `use core::convert;`

- `convert::to_string(x)`. `core` is a keyword
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
