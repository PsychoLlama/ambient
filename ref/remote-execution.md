# Remote Execution

Part of the [Ambient Language Reference](architecture.md).

```
Client                          Server
  |-- Execute(hash, args) ------->|
  |<-- NeedDeps([hash1, hash2]) --|  (if missing)
  |-- Provide([fn1, fn2]) ------->|
  |<-- Result(value) -------------|
```

Remote execution servers are written in Ambient itself on the `Network`
(TCP) and `Execute` (run-by-hash) abilities; the message framing above is
a convention of the example programs, not engine code. Code ships as
canonical object packs — receivers recompute every hash from the bytes.

Executed code runs in an **isolated VM**, and the remote must provide all
ability handlers — nothing proxies back to the caller. Effects reach it
two ways:

- **Host grants** (`ExecuteConfig::grants`): the executing host decides
  which native implementations each isolated VM gets — granting an
  ability means registering the natives its default implementations
  call. The CLI grants Stdio (and Log through it, since Log's defaults
  perform Stdio) — shipped code can print/log on the executing host but
  has no FileSystem, Network, Time, Random, or recursive Execute:
  ungranted performs run their default implementations into stub
  natives that raise a loud, catchable "not wired" exception. This is
  the wasm-style split: the engine is pure; hosts bind capabilities at
  the extern-fn boundary, and different embeddings grant different
  sets.
- **Shipped handlers** (`Execute.run_with(hash, arg, handler)`): a
  first-class handler value travels with the call — its methods are
  content-addressed functions, shipped in packs like any code — and is
  installed at the base of the isolated VM. Method keys make this sound:
  handler and perform match only if both sides derived the same key, which
  means the same ability uuid, the same canonical signature, and the same
  default implementation — a function compiled against version N of an
  ability can never silently dispatch against a handler compiled for
  version N+1. `core::protocol::handler_methods(h)` exposes a handler's
  method hashes so clients can ship its code.

Values cross via `core::protocol::serialize_value`/`deserialize_value`
(bincode of the wire-safe subset: primitives, tuples/lists/records,
enums, function refs by hash, handler values by hash table). Closures,
continuations, maps/sets, and modules do not cross; serializing one is a
runtime error, never a silent `()`.
