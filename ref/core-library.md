# Core Library

Part of the [Ambient Language Reference](architecture.md).

## Core Abilities

The authoritative declarations live in
`crates/ambient-platform/src/platform.ab`; excerpts:

```ambient
ability Time {
  fn now(): Number;               // ms since the Unix epoch
  fn wait(duration: Number): (); // ms
}

ability Random {
  fn seed(): Number;              // 0.0 to 1.0
  fn in_range(max: Number): Number;
}

ability Stdio {
  fn out(message: String): ();   // write a line to stdout
  fn err(message: String): ();   // write a line to stderr
  fn read(): String;             // read a line from stdin
}

// Log is emitted through Stdio, so it declares the dependency: performing
// Log requires Stdio in the effect row, and a handler for Stdio captures
// log lines.
ability Log with core::system::Stdio {
  fn debug(message: String): ();
  fn info(message: String): ();
  fn warn(message: String): ();
  fn error(message: String): ();
}

ability FileSystem {
  fn read(path: String): String;              // UTF-8 text
  fn write(path: String, content: String): ();  // create/truncate
  fn read_binary(path: String): Binary;
  fn write_binary(path: String, data: Binary): ();
  fn exists(path: String): Bool;              // infallible
  fn list(path: String): List<String>;        // sorted entry names
  fn remove(path: String): ();                // file or empty directory
  fn create_dir(path: String): ();            // mkdir -p
}

ability Process {
  fn spawn<I, H>(name: String, init: I, handler: H): Number;
  fn send<M>(pid: Number, msg: M): ();
  fn send_named<M>(name: String, msg: M): ();
  fn self_pid(): Number;              // 0 outside any process
  fn whereis(name: String): Number;   // 0 if no such name
  fn exit(): ();                      // stop after the current reduction
}

// The host process's environment. `var` returns None for an unset
// variable (absence is data, not an exception). `args` is the captured
// argv the CLI composes at startup — index 0 is the program path, the
// rest are the user args after `--` — not live OS state. `set` is
// process-global and best-effort (see below).
ability Env {
  fn var(name: String): Option<String>;
  fn vars(): List<(String, String)>;
  fn set(name: String, value: String): ();
  fn args(): List<String>;            // index 0 is the program path
  fn cwd(): String;
  fn pid(): Number;
}
```

`Process` is the surface of the process model ([processes.md](processes.md)):
named reducer processes with isolated state, message passing, flat
supervision, and reconciliation-based live upgrade under `ambient dev`.
It is **experimental** — see that document for the caveats.

FileSystem failures (missing files, permission errors, invalid UTF-8) raise
catchable `Exception`s, recoverable with
`with { Exception::throw(msg) => ... } handle ...`. Only `exists` is
infallible: it returns `false` when the path can't be inspected.

`Env` reads (and mutates) the process environment. `var`/`vars`/`cwd`/`pid`
read live OS state — a missing variable is `None`, not an exception; only
`cwd` raises (an unreadable working directory). `args` is *not* live OS
state: the CLI captures it at startup — `ambient run <path> -- a b` yields
`[<path>, "a", "b"]`, mirroring Python's `sys.argv[0]` / Go's `os.Args[0]`
(`ambient dev` and the REPL supply an empty argv). `set` is process-global
and best-effort: under edition 2024 it wraps an `unsafe std::env::set_var`,
and since each process runs on its own OS thread, mutating the environment
while another thread reads it is undefined behavior — it is intended for
early startup/config, not concurrent mutation. Exit codes are out of scope.

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
