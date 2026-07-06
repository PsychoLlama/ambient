# Ambient

A content-addressed programming language with algebraic effects, heavily inspired by [Unison](https://www.unison-lang.org/) and [Rust](https://rust-lang.org/).

## Disclaimer

This is almost entirely vibe coded. Don't trust it with anything you love.

I love [PLT](https://en.wikipedia.org/wiki/Programming_language_theory). The [lambda cube](https://en.wikipedia.org/wiki/Lambda_cube) is framed on my nightstand. I've written several toy languages by hand, but a language of this ambition isn't built over a weekend.

Vibing the implementation explores the design and _feel_ of it before committing to ~~months~~ [a lifetime](https://www.youtube.com/watch?v=XZ3w_jec1v8) of work.

I built this to explore the idea, not to use it in production.

## Features

Two key ideas:

- **Hashing:** Every function is identified by the hash of its implementation and its dependencies. Change the implementation, get a new hash.
- **Effects:** Functions are pure. Effects are provided by the host environment as algebraic effects ("abilities").

Together, these properties unlock a unique set of features.

### Perfect Caching

The build cache is an append-only and self-verifying object store akin to git.

- Fast, incremental compilation.
- Cache computations that didn't change between runs. For example, unit tests.
- Share the cache with CI or host it for local builds.

### Live Upgrades

Load new symbols into a program while it's running and incrementally cut over to the new version. Old code and new code can safely coexist during the upgrade.

If you've used HMR in web development or interactive repls in Lisp, it's the same idea, but without stale references or environment pollution.

### Remote Functions

Send and receive closures between running programs. Build remote repls, GraphQL-style servers, or transfer work across a cluster.

> [!NOTE]
> This is opt-in, controlled by your program via the `Execution` API.

### Programmable Effects

Effects are part of the type system. They can be captured, replaced, mocked, or blocked entirely. Programs can define custom effects (abilities) composing other effects.

Examples:

- **Sandboxing:** Limit what APIs are callable from libraries or remote functions.
- **Mocking:** Replace effects in tests without rewriting the code.
- **Record & Replay:** Store effects and their outputs. Save it, play it back later.
- **Overrides:** Rewrite effects before they're performed.

## Taste of the language

```ambient
// Effects are explicit capabilities ("abilities"), tracked in types.
pub fn run(): () with platform::Log {
  greet("world");
}

// Private functions infer their abilities. No annotation needed.
fn greet(name: string) {
  platform::Log::info!("Hello, ${name}!");
}
```

```ambient
// Nominal types + traits. Operator traits (Add, Eq, Ord, ...) are prelude.
unique(B3C4D5E6-F7A8-9012-BCDE-F12345678902) struct Vec2 { x: number, y: number }

impl Add for Vec2 {
  fn add(self, other: Vec2): Vec2 {
    Vec2 { x: self.x + other.x, y: self.y + other.y }
  }
}

fn example(): bool {
  Vec2 { x: 1, y: 2 } + Vec2 { x: 3, y: 4 } == Vec2 { x: 4, y: 6 }
}
```

Effect handlers are delimited continuations. Mock any capability in tests, sandbox untrusted code, or intercept and transform operations:

```ambient
fn roll_dice(): number {
  math::abs(platform::Random::in_range!(6))
}

fn run(): () with platform::Random, platform::Log {
  let roll = handle roll_dice() {
    platform::Random::in_range(max) => {
      resume(4) // Chosen by fair dice roll. Guaranteed to be random.
    }
  };

  platform::Log::info!("Rolled ${roll}");
}
```

## Usage

There are no binaries published yet. It must be compiled from source:

```bash
cargo build --workspace
```

```bash
ambient init my_project          # scaffold a package
ambient run my_project           # compile + run (persists to .ambient/store)
ambient store show run           # inspect any function: deps + disassembly
ambient repl                     # interactive REPL
ambient dev my_project           # live-upgrade loop (hot-swaps processes, keeping their state)
ambient lsp                      # language server (see ambient.nvim/)
```

Every build lands in a per-package content-addressed store (`.ambient/store/`) laid out git-style. `ambient store` gives you `stats`, `ls`, `show` (with disassembly), `deps`, `verify`, and `gc`. `ambient compile` emits a single-file `.ambient` artifact (the same object format plus name bindings and an entry point) that `ambient run` executes after recomputing every hash from content.

See the `examples/` directory for runnable programs.

## Why Not [Unison](https://www.unison-lang.org/)?

Unison blew my mind in 2019. I love it and advertise it to anyone who'll listen. It's a great language and infinitely better than what I can build.

So why create a new language?

- **Target Environment:** I desperately wanted to try their ideas in web apps and embedded in web servers through FFI. Unison doesn't support it.
- **Scratch Files:** Unison takes a radical stance by doing away with the file system. You edit the codebase by loading and unloading code from a scratch file. It's an expensive gamble. I wanted to prove it was possible to keep the features _and_ the file system.

This is why `unique(...) struct` requires a literal UUID. Unison can create nominal types under the hood, but ambient files have to be self contained. It needs a way to uniquely identify the type across moves and remote functions.
