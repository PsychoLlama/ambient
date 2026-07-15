# Core Library

Part of the [Ambient Language Reference](architecture.md).

## Core Abilities

The authoritative declarations live in
`crates/ambient-platform/src/platform.ab` — the platform bindings interface
declares every core ability (Time, Random, Stdio, Log, FileSystem, Process,
Env, Tcp, Execute) with doc comments on each method. See
[abilities.md](abilities.md) for how those declarations become in-scope
abilities, and [live-upgrade.md](live-upgrade.md) for the live-upgrade
set (`Live`, `State`, `Task`, `Drain`, `Deploy`).

## Native functions (`extern fn`)

The host boundary. A host operation — UTF-8 string length, f64 square
root, persistent-map insert, a stdout write — is an **extern fn**: a
body-less signature declared in Ambient source, implemented by the host.

```ambient
/// Number of characters in the string.
pub extern fn length(value: String): Number;
```

The signature (typed params, declared return, doc comment) is owned by the
`.ab` module. The host binds an implementation through a `NativeRegistry`,
keyed by _(module path, name)_ — and supplies the function's **stable
UUID**, which is its content identity. An extern fn compiles to a tiny
canonical _native object_ encoding exactly `(uuid, param_count)`; callers
link to its hash like any function, so packs, remote execution, and
first-class function values need no special cases. Renaming a declaration
re-keys the host binding (a loud build error until the host catches up)
but never moves a hash — the name is not in the encoding. The converse
discipline: a UUID pins a meaning forever; changing semantics mints a new
UUID.

The contract is enforced in both directions before anything compiles:
every declaration must have a binding (with matching arity), and every
binding must name a declaration. A remote host that receives code calling
a native it does not implement fails loudly at the call
(`UnboundNative`, naming the UUID) — never a silent misbehavior.

Extern fns take no `with` clause and no ability channel — in the type
system they are effect-free. Core's natives really are pure value
transformations. Effectful host operations exist too (the platform's
`stdio_out`, `fs_read`, ...), but they are module-private to
`core::system`, so the only code that can call them is that module's
ability method bodies — the default implementations of `Stdio`,
`FileSystem`, and friends. A runtime host may register a **VM-invoking**
implementation directly on a VM (the platform's `state_update` is one):
it receives the calling VM and can run function values it was handed,
reentrantly and delimited at the invoke boundary — performs inside the
invoked function see only handlers installed within it, falling through
to default implementations otherwise. Effect _tracking_ therefore lives entirely at
the ability layer: user code can only reach an effectful native through a
perform, which the checker accounts for. Anything effectful belongs to an
ability; an effectful extern fn is its implementation detail, guarded by
visibility.

The engine's own bindings for `core` live in
`crates/ambient-engine/src/natives/core/` under the reserved UUID block
`FFFFFFFF-FFFF-FFFF-FFFE-…`. Embedders bind their own module trees the
same way (`BuildOptions::natives` + `Vm::register_natives`) — the same
mechanism, different UUIDs.

## Standard Functions

Option, Result, List, String, Number, and Binary expose their combinators
and predicates as inherent methods — the canonical core API — so pipelines
read receiver-first (head/get/first/last return Option — no sentinel
values):

```ambient
[1, 2, 3].filter((x) => x % 2 == 1).map((x) => x * 10).fold(0, (a, x) => a + x)
Some(20).map((v) => v * 2).unwrap_or(0)
Ok(5).map((v) => v + 1).ok().is_some()
"hello".to_upper().chars().length()
```

The method names, by type:

```ambient
// Collections
List::{map, filter, fold, any, all, sum, get, head, tail, first, last,
       length, is_empty, reverse, slice, append, concat}
// List methods gated on an element-type bound: Eq unlocks contains/index_of,
// Ord unlocks sort/min/max. `sort` is a stable O(n log n) merge sort by
// `Ord::cmp` — the one sort (there is no separate structural native).
List<T: Eq>::{contains, index_of}   List<T: Ord>::{sort, min, max}
Map::{get, insert, remove, contains, length, is_empty, keys, values}
Set::{insert, remove, contains, length, is_empty, union, intersection,
      difference, to_list}

// Options
Option::{map, and_then, or_else, is_some, is_none, unwrap_or}

// Results
Result::{map, map_err, and_then, ok, is_ok, is_err, unwrap_or}

// Strings
String::{length, is_empty, concat, contains, split, trim, slice, chars,
         replace, starts_with, ends_with, to_upper, to_lower, index_of,
         repeat, reverse, pad_start, pad_end, lines}

// Numbers
Number::{clamp, sign, is_negative, is_positive, is_zero, lerp, abs, sqrt,
         floor, ceil, round, trunc, pow, min, max, sin, cos, tan, asin,
         acos, atan, atan2, ln, exp, log10, log2}

// Binary
Binary::{to_list, length, is_empty, get, slice, concat}
```

Operations with no single receiver are **associated functions** on the type,
called `Type::name(...)` — the low-level externs they delegate to are
module-private, so the type is the whole public surface:
`List::range(start, end)`, `Map::empty()`, `Set::empty()`,
`Binary::empty()`, and `String::join`. (`Binary::empty()` stays an associated
function rather than a `From` impl: an empty list literal can't infer its
element type through argument-directed `From` selection.)

Conversions between types are `From` impls (prelude-exported, always in
scope): `String::from(n)`/`String::from(b)` render a `Number`/`Bool`, and
`Binary::from(bytes)`/`Binary::from(s)` build a buffer from a `List<Number>`
or a string's UTF-8 bytes. Argument-directed selection lets both `Binary`
constructors coexist (their argument heads differ), and each is equivalently
spelled `value.into()` where the target type is known. `Binary::to_string`
stays a receiver method: it returns `Option<String>` (decoding can fail), so
it is not `From`-shaped.

The one operation that stays a qualified module free function is
`core::option::flatten(opt)` — its receiver would be `Option<Option<U>>`,
inexpressible in an `impl<T> Option<T>` block. The receiver-less utility
modules likewise stay module functions: `core::convert::to_string`,
reflection (`core::reflect::{tag, payload}`), and the wire protocol
(`core::protocol::{serialize_value, deserialize_value, closure_hash,
closure_captures, handler_methods, hex_to_binary, binary_to_hex}`).

Fallible parsing goes through the prelude `TryFrom<String>` impls instead:
`Number::try_from("4.2")` and `Bool::try_from("yes")` return
`Result<_, String>` (the error carries the rejected input), equivalently
spelled `s.try_into()` where the target is known. The low-level parser
externs (`parse_number`, `parse_bool`) are module-private in
`core::convert` — the impls are their only callers.

## Container Key Semantics

`Set<T>` and `Map<K, V>` compare keys **structurally** — by the runtime
`Value`'s own byte-level equality (`ambient-ability`'s `MapValue`/`SetValue`,
keyed by `Value::eq`), preserving insertion order. This is deliberate and
fixed: it is **not** routed through a `K: Eq`/`K: Ord` trait bound, and a
user's `impl Eq`/`impl Ord` for a key type has **no effect** on container
membership, deduplication, or lookup.

That follows from the language's core stance: **ambient values are plain
data**. Nominal identity (a `unique(<uuid>) struct`'s UUID) is a
_compile-time_ concept — at runtime a `Money` is erased to its record, so two
`Money` values are equal to a container exactly when their fields are. A
custom `Eq` impl governs `==`, operator dispatch, and trait bounds (including
`List<T: Eq>::contains`, which _does_ dispatch through the impl); it never
governs how a `Set`/`Map` hashes or compares its keys. The two notions of
equality are intentionally separate, and containers pick the structural one.

The consequence — worth stating plainly — is that a **coarser** custom `Eq`
diverges from container keying. Take a case-insensitive identifier:

```ambient
unique(D0D0D0D0-0000-0000-0000-000000000001) struct CiName { text: String }

impl Eq for CiName {
  // Case-insensitive: "Ada" and "ada" are equal values.
  fn eq(self, other: CiName): Bool {
    self.text.to_lower() == other.text.to_lower()
  }
}

let a = CiName { text: "Ada" };
let b = CiName { text: "ada" };

a == b                        // true  — dispatches through the Eq impl
[a].contains(b)               // true  — List::contains uses the Eq bound
Set::empty().insert(a).insert(b).length()   // 2 — structural keys: "Ada" ≠ "ada"
```

`a == b` is `true` (the impl collapses case), but the `Set` holds two
elements because their underlying records differ. If a program needs
`Eq`-consistent keys, it must normalize before inserting (store
`CiName { text: self.text.to_lower() }`) — the container will not do it.

This is the same class of hazard as any `Ord`-ordered structure surviving a
handler edit (see [traits.md](traits.md), "Why this survives live upgrade"):
container behavior is fixed by the _data_, not by an impl that a later build
could change. Making structural keys the definition, rather than a
trait-dispatched key, is what keeps `Set`/`Map` contents stable across live
upgrades and wire transfers — an impl edit can never silently re-bucket an
existing collection.
