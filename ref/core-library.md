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

## Standard Functions

Option, Result, and List expose their combinators and predicates as inherent
methods — the canonical core API — so pipelines read receiver-first
(head/get/first/last return Option — no sentinel values):

```ambient
[1, 2, 3].filter((x) => x % 2 == 1).map((x) => x * 10).fold(0, (a, x) => a + x)
Some(20).map((v) => v * 2).unwrap_or(0)
Ok(5).map((v) => v + 1).ok().is_some()
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
```

Two combinators have no method form and stay qualified module functions:
`core::collections::List::range(start, end)` (no receiver) and `core::Option::flatten(opt)`
(its receiver would be `Option<Option<U>>`, inexpressible in an
`impl<T> Option<T>` block). Strings and conversions remain module functions:

```ambient
// Strings
String::split, String::join, String::trim, String::contains, String::length

// Conversion (parsers return Option)
to_string, parse_number, parse_bool
```
