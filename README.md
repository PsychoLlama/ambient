# Ambient

A pure, content-addressed programming language. Heavily inspired by [Unison](https://www.unison-lang.org/).

> [!WARNING]
> Vibe coded. Breaking changes and no backwards compatibility.

## Overview

Code is content-addressed and serializable. Effects are abilities declared in types.

Everything else comes from those properties.

## Features

- **Dev speed:** Builds, tests, lints, type checks, and `comptime` all cache as merkle trees.
- **Binary size:** Only code that's used makes it into the build. Duplicate functions point to the same address.
- **Compile-time code:** Hygienic macros and build hooks handle anything you'd need from `build.rs` or a web bundler.
- **Serializing:** Functions and values are serializable to disk or over the network.
- **Grants and sandboxing:** Restrict what effects are available to code and packages, enforced at compile time.
- **Loading:** APIs for live reloading, (trusted) remote functions, and upgrades that preserve system state.
- **Record and replay:** Observe effects, replay them in tests, permute them in simulations.
- **Host IO:** Targets and packages extend the language with native bindings to the host.

## Targets

- WebAssembly: node, browsers, and other wasm runtimes.
- Native: a [Cranelift](https://cranelift.dev/) backend for standalone binaries.
- Embedding: as a language within other tools (such as [nvim.rs](https://github.com/PsychoLlama/nvim.rs)).

## Usage

- **CLI:** binaries for macOS and Linux in [GitHub releases](https://github.com/PsychoLlama/ambient/releases/latest).
- **API:** only available as a git dependency. `crates.io` shouldn't suffer for my slop.

Working language demos are in [the `examples/` directory](./examples):

```sh
cd examples/hello && cargo run -p ambient -- run
```

See [guide](docs/guide.md) for a tour of the language.
