# Ambient

A content-addressed programming language with algebraic effects, inspired by
Unison, Rust, TypeScript, and Nushell.

Every function is identified by the hash of its implementation and its
dependencies. That buys perfect incremental compilation, deduplication, and —
the long-term goal — live program upgrades and remote execution by shipping
hashes instead of source: a client and server that share a content-addressed
store can exchange functions as data, request missing dependencies by hash,
and hot-swap behavior by pointing at a new root.

Unlike Unison, the filesystem stays the source of truth: programs are plain
`.ab` text files, and language semantics map 1:1 to what's on disk.

## Taste of the language

```ambient
// Effects are explicit capabilities ("abilities"), tracked in types.
pub fn run(): () with Console {
  greet("world");
}

// Private functions infer their abilities — no annotation needed.
fn greet(name: string) {
  platform::Console::print!("Hello, ${name}!");
}
```

```ambient
// Nominal types + traits. Operator traits (Add, Eq, Ord, ...) are prelude.
unique(b3c4d5e6-f7a8-9012-bcde-f12345678902) type Vec2 { x: number, y: number }

impl Add for Vec2 {
  fn add(self, other: Vec2): Vec2 {
    Vec2 { x: self.x + other.x, y: self.y + other.y }
  }
}

fn example(): bool {
  Vec2 { x: 1, y: 2 } + Vec2 { x: 3, y: 4 } == Vec2 { x: 4, y: 6 }
}
```

Effect handlers are delimited continuations — mock any capability in tests,
sandbox untrusted code, or intercept and transform operations:

```ambient
handle {
  platform::Console::print!("hello");
} {
  platform::Console::print(msg) => {
    platform::Console::print!(core::string::concat("[LOG] ", msg));
    resume(())
  }
}
```

## Getting started

```bash
cargo build --workspace          # or: nix develop

ambient init my_project          # scaffold a package
ambient run my_project           # compile + run (persists to .ambient/store)
ambient store show run           # inspect any function: deps + disassembly
ambient repl                     # interactive REPL
ambient dev my_project/src/main.ab   # hot-reload loop
ambient lsp                      # language server (see ambient.nvim/)
```

Every build lands in a per-package content-addressed store
(`.ambient/store/`) laid out git-style: one file per canonical object,
named by the blake3 of its bytes, so everything on disk is self-verifying.
`ambient store` gives you `stats`, `ls`, `show` (with disassembly), `deps`,
`verify`, and `gc`. `ambient compile` emits a single-file `.ambient`
artifact — the same object format plus name bindings and an entry point —
that `ambient run` executes after recomputing every hash from content.

The `examples/` directory has ~25 runnable programs, from `hello` to a
TCP echo server and a remote-execution client/server pair — all exercised
by the test suite (`cargo test -p ambient-cli --test examples`).

## Repository layout

| Path | What it is |
|------|------------|
| `crates/ambient-engine` | Type inference, compiler, bytecode VM, content-addressed store |
| `crates/ambient-parser` | Hand-written lexer + parser (CST → AST) |
| `crates/ambient-cli` | The `ambient` binary |
| `crates/ambient-lsp` | Language server |
| `crates/ambient-core` / `-ability` / `-platform` | Ability descriptors, runtime values, host abilities |
| `ref/` | Language design docs (`architecture.md` is the source of truth) |
| `examples/` | Runnable example packages with golden-tested output |
| `tree-sitter-ambient/`, `ambient.nvim/` | Editor tooling |

## Development

```bash
just check    # format check + clippy + build + all tests; must pass before commit
```

Language regressions are caught at three levels: unit tests in the engine,
golden tests over every example program, and content-addressing invariant
tests (`crates/ambient-parser/tests/content_addressing.rs`) that pin the
hash semantics: same source → same hash, declaration order never matters,
body or dependency changes always ripple.

## Status

Experimental but coherent: packages, modules and imports (item,
whole-module, and a standard library compiled as ordinary Ambient
modules), traits with static content-addressed dispatch, enums with
constructors and typed patterns, ability inference and enforcement,
delimited-continuation handlers, and a persisted self-verifying store
with introspection tooling all work today — plus a REPL and an LSP.

Abilities are content-addressed like functions: identity is the blake3
hash of the canonical interface, `ability` declarations work in-language,
and remote execution over TCP ships *effectful* functions — the executing
host grants capabilities (wasm-style), and handlers themselves travel by
hash (`Execute.run_with`). See `ref/architecture.md` § Future Work for
what's next (finishing the platform-binding split, cross-module ability
imports, WASM target).
