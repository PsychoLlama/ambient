# logging

Structured logs as an ability: an Ambient package with no native code. The root `AGENTS.md` holds what spans crates.

## Invariants

- Code that logs performs `log` and nothing else. Where a log goes is decided by whoever provides `log`, never by an argument passed down.
- Nothing here reaches a world. A formatter (`ansi.line`, `ndjson.line`) gives text and is handed the moment; the program's handler writes it and reads the clock.
- A handler is a plain `fn(entry: Log): null`. What chooses or fans out logs is a function from handlers to a handler, so it composes with any other.
- Origin is dynamic scope: `within` provides `log` again and performs the binding it replaced. No log carries a logger.
- A level is a tag of `level`, and `Level` their union, so a match over one is exhaustive. Order is `level.rank`, stated once.
- A context is a `Map<json.Member>`, so JSON writes it as it is. A formatter that wants its own order of members writes them one by one (`ndjson.line`).

## Testing

- `ambient test` here, which `crates/ambient/tests/examples.rs` runs.
- `examples/logs` is a program that uses the package under the `system` target.
