# Core Library

Part of the [Ambient Language Reference](architecture.md).

## Core Abilities

The authoritative declarations live in
`crates/ambient-platform/src/platform.ab` — the platform bindings interface
declares every core ability (Time, Random, Stdio, Log, FileSystem, Process,
Env, Network, Execute) with doc comments on each method. See
[abilities.md](abilities.md) for how those declarations become in-scope
abilities, and [processes.md](processes.md) for the (experimental) `Process`
ability.

## Native functions (`extern fn`)

The other half of the host boundary. Abilities are for _effects_; a pure
host operation — UTF-8 string length, f64 square root, persistent-map
insert — is an **extern fn**: a body-less signature declared in Ambient
source, implemented by the host.

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

Extern fns are pure by construction: no `with` clause, no VM access, no
ability channel. Anything effectful belongs to an ability.

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
       length, is_empty, reverse, sort, slice, append, concat}

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

Operations with no single receiver stay qualified module functions:
`core::collections::list::range(start, end)`,
`core::option::flatten(opt)` (its receiver would be `Option<Option<U>>`,
inexpressible in an `impl<T> Option<T>` block),
`core::primitives::string::{join, from_number, from_bool}`, and
`core::primitives::binary::from(bytes)`. `Map` and `Set` expose their
whole surface as module functions (`core::collections::map::{empty, get,
insert, remove, contains, length, keys, values}`,
`core::collections::set::{empty, insert, remove, contains, length, union,
intersection, difference, to_list}`), as do the conversions
(`core::convert::{to_string, parse_number, parse_bool}` — parsers return
Option), reflection (`core::reflect::{tag, payload}`), and the wire
protocol (`core::protocol::{serialize_value, deserialize_value,
closure_hash, closure_captures, handler_methods, hex_to_binary,
binary_to_hex}`).
