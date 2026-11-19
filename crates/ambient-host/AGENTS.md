# ambient-host

What loads and runs a compiled program, with no compiler in it. The root `AGENTS.md` holds what spans crates; bullets name modules as `ambient` gives them again (`ambient::core`, `backend::native`, `host`).

## Invariants

- Native code reaches its program only through `native::Running`, which each backend's dispatch implements. What it loads is held until its result is encoded, so nothing of the program is borrowed while it runs.
- Native code calls back through `Running::call`, only a function read for the call it is in (`value::Live`): a decoded function is a copy that lost its cells and handlers. The runtime entry is `amb_call_among`, or `amb_call_back` in a program linked without suspension.
- What needs a world is the `system` package (`system/`, `docs/spec/targets.md`), installed beside the library; `ambient` holds nothing that does. The command-line target is `system.cli` (`system/lib.ab`); its native side is `host/`, where every handler goes through `host::World`, and one that takes time begins `host::Work` and is answered later. The real process waits on threads that send what they got (`host/pending.rs`, `host/network.rs`); only the program's thread touches a value. Tests run programs over `support::scripted::Scripted`, never the real process.
- `system.code` is native code over an engine with an empty workspace (`host/code.rs`): a pack leaves nothing out, so compiling one needs nothing else. It compiles for the backend `Running::backend` names (`backend::named`), and what `code.load` takes is `Parts::encode`. Loading is the host crate's; packing and compiling need the compiler, and are added to its handlers (`host::compiling`).
- An executable is a launcher with a program after it (`launch::Built`): encoded parts, the names and places a report takes (`launch::Frames`), a length, and a mark. A launcher looks for the mark in its own file as it starts (`launch::own`); `launch::started` is the one way a compiled program is run and reported, built or not.
- A definition holds its own hash (`DEFINITION_HASH`), so a host reads a function value as a hash and captures (`value.rs`), and makes one of a definition the program has (`native::Inside`); what it captures is trusted.
- A native `Program` is units: each `Program::add` lays out a block (`statics::plan` with a `Base`) that refers to what the program already has by address. `Engine::link_to_load` makes the parts.
- `statics::plan` names a part's root or definition (`Field::Root`, `Field::Definition`) and an extern (`Field::Native`) without saying where; each loader resolves them.
- `Parts::encode` is compiled code for a host with no compiler (`ambient-host/src/parts/encode.rs`), refused by another backend, format, or build of the host. Bump its `FORMAT` when `Parts`, a layout, the calling convention, or a runtime entry changes.
- A value has one encoding (`exchange.rs`); `decode` accepts only what `encode` produces. A host sees inside an iterator (`value::Source`), and only one that keeps what its source keeps (`Source::keeps`) is decoded or made in a program: the runtime steps without looking.
- `core::Tree::resolved` defines every hash: the tree's encoding with slots replaced by hashes and unions ordered by that. `Tree::encode` is the stored form, which names can order; never hash it. Changing either or the node set means bumping `FORMAT`.
