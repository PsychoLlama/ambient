## Structure

- `spec/`: source of truth for the language design. The implementation follows it; change the spec first.
- `crates/ambient`: the implementation, a library plus an empty binary. Module docs name the spec page each module implements.
- `crates/ambient-runtime`: what compiled code calls. Built natively into the compiler, and for wasm32 by `crates/ambient/build.rs` as one relocatable object.

## Invariants

- Text is read by `ambient::syntax` into the same surface trees the builders make, through the builders. Tests still build programs with `ambient::builder`; only tests of the syntax itself, and of whole programs on disk, use text.
- Every stage result is stored under a key derived from the content of its inputs (`engine/mod.rs`). A stage's compute function reads nothing that is not in its key.
- The compiler itself is an input no key names: `build.rs` hashes its source into `AMBIENT_BUILD`, and `Engine::stage_if` stores under the key and that.
- A comptime call takes only abilities its package is granted (`Sources::grants`), checked at the call before it runs; the check key of a definition with comptime calls covers the grants.
- Checking is re-entrant: a comptime call compiles and runs its callee mid-check (`engine/run.rs`). The check key covers the callee's hash, which is what keeps the result a function of the key.
- A result that observed the world is stored with what it observed and reused through `Engine::stage_if`, which looks again. Only native code is outside a key, so a trace is the native calls a run made.
- Names, docs, and declaration order never reach a hash, but for one case: an ability, a key, an extern, and a tag are told apart by module path and name (`Symbol::identity`), which their trees hold and their stage keys cover. In a core tree names otherwise decide one thing: the stored order of a union's members, and so slot numbering.
- An operation is a `core::Primitive` and a runtime entry of the same name; `types::Env::operation` types it for both checkers, and the checker picks which one an operator means (`checker/operators.rs`). Adding one touches those, `core/cost.rs`, and `lir/steps.rs`, and no backend.
- A backend only translates `lir`; anything about semantics belongs in `lower.rs` or the runtime, so both backends get it.
- Layouts and the calling convention are defined once, in `ambient-runtime`; never restate an offset in a backend.
- Every function's first parameter is its bindings: a record keyed by ability, or null. The checker tracks what is bound (`checker/abilities.rs`); lowering only threads the record (`lower/abilities.rs`).
- A record holds a method as a closure of its own kind over a function the checker writes (`checker/methods.rs`), with a null capture for the record; `amb_record_get` fills it in on a copy, so a record never holds itself. Anything that copies a field out of a record goes through `record::read`.
- A method call (`surface::Expr::Send`) means a field of a record, or for a built-in shape the public function of that name in its module of the standard library (`checker/sends.rs`); nothing else adds a method. Resolution cannot know the type, so the engine gathers the function for every shape into `Env::methods` and the check key (`engine/environment.rs`).
- A list is a view of a buffer that later lists share (`ambient-runtime/src/list.rs`); a push onto the list that ends its buffer writes into the room after it. Read a list's members through `list::members`, never `words`.
- An iterator is a shape of its own (`ambient-runtime/src/iterator.rs`): a function that steps, or a list or a range the runtime steps. `core::Expr::Step` asks for the function and calls it, or has the runtime advance (`Lowering::step`); everything else about iterators is library code over a step (`library/iterator.rs`).
- A tag (`DefinitionKind::Tag`) resolves to a type definition: the literal type of the tag its identity names. A name in value position falls back to the type namespace, and the checker gives the value only for such a definition (`Checker::tag`).
- A key is a definition in a namespace of its own (`Namespace::Key`); a record names one where it names an ability (`Key::Ability`), and `Env::holds` is the one rule for what may be held under it, which both checkers ask.
- A merge names the keys whose methods receive the merged record (`core::Expr::Merge::methods`): exactly those its type lists as methods. The runtime binds every other method to the record it came from, so no method meets a receiver nothing checked.
- A row is ordered by identity wherever it reaches a core tree, never by the order it was written in.
- A program is `Engine::program(entry, target)`: a root function the engine writes (`engine/program.rs`) and then checks, hashes, and compiles like any other.
- Compiled code reaches native code only through `amb_native` and an index the loader fills in; a host supplies `Natives` by package and the extern's module path and name.
- Native code reaches its program only through `native::Running`, which each backend's dispatch implements. What it loads is held until its result is encoded, so nothing of the program is borrowed while it runs.
- A task suspends by unwinding with the context's mode set, and resumes by its functions being called again (`lower/resume.rs`, `ambient-runtime/src/task.rs`). A host that sees the entry unwind while suspending calls `amb_schedule` and the entry again.
- A program is linked closed (`Engine::link`) or open to loading (`Engine::link_open`, `Parts::open`); only an open one is loaded into. A closed program in which no core tree `suspends` is compiled `still` (`lower::Form`, `engine::compile::Mode`): no operation is a point.
- A function with no point (`lir::Function::points`) is left as lowered and emitted without what resuming takes; a backend must not read its resume target.
- A tail call unwinds with the call pending in the context, arguments and all (`lower/tail.rs`, `Op::Tail`), and every other call is followed by `Op::Bounce`, which makes it (`ambient-runtime/src/task/tail.rs`). Whatever calls compiled code from outside goes through `amb_start`, never a code address.
- A task's body is a closure that takes nothing; the checker makes one for each branch, spawn, and cleanup (`checker/tasks.rs`), so they capture locals and bindings as any closure does.
- A using (`surface::Expr::Using`) is gone after resolution (`engine/resolve/expr.rs`): it binds the value, reads what it holds under `ambient.cleanup.dispose`, and registers a call of that as a `Cleanup`. A surface tree never holds a `Cleanup` itself.
- Whatever must happen on every way out of a block (cleanups, closing a group) is lowered as a `Try` whose recovery holds the unwinding, does the work, and rethrows (`lower/tasks.rs`).
- A trace is gathered only while a fault unwinds: lowering wraps each operation a fault can leave in a `Try` that notes its position (`core::Expr::positions`), so code that does not fault pays nothing.
- A captured trace is read off a suspension (`ambient-runtime/src/task/capture.rs`): the task suspends, each frame it saves says where its function was, and it resumes at once. A point's frame is the position of the `Trace` that wraps it (`lower/resume.rs`).
- What a node costs inside a budget is defined once, on the core tree (`core/cost.rs`). Lowering places that charge before the node and makes no other.
- Metering is a mode of lowering (`lower/meter.rs`): on for the body of a budget, and for the whole of a function's metered form. A definition holds both forms' code under one hash; a call made in metered code reaches the metered form, so code outside a budget makes no charge. Only what can run inside a budget gets a metered form (`Engine::metered`): what a budget's body names, every function made a value, and what those reach. Both forms read one definition, so the metered form's constants begin with the plain form's.
- A function's locals live in memory in both backends: a Cranelift stack slot, and in wasm a frame on the runtime's stack (`__stack_pointer`, which the runtime exports). Locals as SSA values across suspension points make the engine's compile time quadratic.
- A tree nests at most `core::DEPTH` levels (`core/depth.rs`): the builder's validation, `Engine::check`, and decoding each refuse a deeper one. Walks recurse, so every `Engine` entry that touches a tree runs inside `lower::deep`.
- Lowered code nests once per bound value, so whatever makes, walks, or drops a `lir::Function` runs inside `lower::deep`, on a stack with room for it.
- Compiled code is position-independent: it reaches definitions through slots and the runtime through entry points, never by address.
- A name reaches a definition through a module's members (`engine/scope.rs`): its own definitions and its re-exports, which keep the symbol of what they alias. Another package is reached only through its library module and the modules that lists.
- The standard library is a package installed into a workspace on request (`library/`). Its modules are `.ab` source, embedded and read at install; only its library module is built, since fault kinds and budget limits are tags named by the runtime, never restated.
- A method of a built-in shape is a public function in `library/<shape>.ab` whose first parameter is `self`. Generic library code narrows with a match and a binder, never a cast to a type parameter, which would make it need type arguments as it runs.
- The reader (`syntax/`) resolves nothing: it tracks only the locals of the function being read, to tell `record.field` from `module.definition`. Statements, early returns, `if`, and the operators the language lacks are arranged into existing nodes (`syntax/parse/block.rs`, `expr.rs`); do not add a surface node for something an arrangement can say.
- Names the reader makes up start with `__`, which no program may write.
- The command-line target is `ambient.system.cli` (`library/system.ab`); its native side is `host/`, where every handler goes through `host::World`. Tests run programs over `support::scripted::Scripted`, never the real process.
- A package on disk is read through `project::Files` (`project/`), and `cli::Session` is handed its files, store, and world, so `tests/cli.rs` drives the command line in memory.
- The core checker (`verify/`) accepts what `checker/` makes and nothing looser; `support::run` verifies every definition it runs. Typing rules live in `types::Env`, which both checkers ask.
- A received definition (`engine/pack.rs`) is an `Open` under a made-up symbol: it is verified, then hashed, compiled, and linked like any other. It has no surface tree and never reaches the store.
- A definition holds its own hash (`DEFINITION_HASH`), so a host reads a function value as a hash and captures (`value.rs`), and makes one of a definition the program has (`native::Inside`); what it captures is trusted.
- A native `Program` is units: each `Program::add` lays out a block (`statics::plan` with a `Base`) that refers to what the program already has by address. `Engine::link_to_load` makes the parts, every function with a metered form.
- A wasm program is one module: its compiled functions and the pieces of the runtime they and its host reach (`backend/wasm/link/`, `spec/linking.md`). A compiled body leaves room where it calls the runtime and says where (`backend/wasm/code.rs`), since no function has an index until the module is put together.
- What a piece of the runtime refers to is read from the object's relocations (`link/object.rs`), never listed by hand. `tests/footprint.rs` holds the size of a program that does nothing to a budget.
- The runtime is `no_std`, and in wasm has its own allocator (`heap.rs`), unlocked state (`state.rs`), and a panic handler that traps. Use `core` and `alloc` there, never `std`, and no `format!` on a path every program reaches.
- Linking cuts the standard library's panic functions to a trap by name (`PANICS`, `link/object.rs`), since they format before the handler runs.
- Wasm loads a `wasm::unit` module over a `wasm::open_module` program (`backend/wasm/units.rs`, wasmtime), with what loading keeps in the store's `State` so a host call can load too. The unit imports each definition's address by hash (`known`), zero meaning it lays out its own; modules describe themselves through exported globals (`root:`, `definition:`, `metered:`, `native:`).
- A unit is linked for the pieces its target has (`wasm::unit(parts, has)`, `link::For::Unit`): it imports those by hash (`piece`), carries the rest, and exports what it carries (`piece:<hash>`, `state:<hash>`). Its pieces' data is in a block its `init` takes, reached through globals (`link/unit.rs`), and is never given back.
- A state is imported as an address by hash (`state`), zero meaning the unit uses its own copy. Only writable data is state; constants are always carried.
- `statics::plan` names a part's root or definition (`Field::Root`, `Field::Definition`) and an extern (`Field::Native`) without saying where; each loader resolves them.
- A loaded block is a unit (`ambient-runtime/src/unit.rs`): it starts with a count, and a word naming the unit precedes each definition, fixed object, and constant descriptor in it. Closures, cells, type arguments, and dependent units hold the count; `Program::reclaim` drops units at zero.
- `Op::Copy` of a local, capture, or constant takes a reference (`lir::Value::counted`); a loaded unit's strings are counted, not pinned.
- A function type with type parameters is `Type::Generic` around a function type, its parameters `Type::Bound` indices; `Reference::Parameter` is only ever a parameter of the definition being checked. A value of one is instantiated before it is called (`core::Expr::Instantiate`, `amb_instantiate`), which copies the closure only if its definition says it is handed type arguments (`DEFINITION_TYPES`).
- A generic function that needs its type arguments reads them from its closure, after its captures and bindings (`lower/types.rs`, `ambient-runtime/src/scope.rs`), so it is never called through its slot. Which functions need them is `Engine::handed`, the least answer over what hands them on to what; lowering is told through `lower::Form`, and the compile key covers it.
- How a recursive type varies with its parameters, and whether unfolding it ends, is worked out when an environment is gathered (`Env::settle`, `types/variance.rs`). Two uses of one definition are compared by arguments first, and one that is not `finite` is never unfolded against itself.
- Subtyping is decided only when checking (`types/relate.rs`). The runtime compares two types for sameness alone, for casts to cell types (`ambient-runtime/src/same.rs`); a cast to a function type compares one word, the name of the type (`backend/identity.rs`), which a definition carries.
- A function whose type depends on type arguments carries a descriptor of it instead of a name (`DEFINITION_DESCRIBED`, `link::says`), and only if it is handed them. A cast reads it in the arguments the closure holds (`closure::described`).
- A native `Program` keeps a list of its live cells and empties them when it drops (`ambient-runtime/src/cell.rs`), since counting never reclaims a cell that reaches itself. In wasm the instance's memory goes as a whole.
- `Parts::encode` is compiled code for a host with no compiler (`engine/parts.rs`), refused by another backend or format. Bump its `FORMAT` when `Parts`, a layout, the calling convention, or a runtime entry changes.
- A value has one encoding (`exchange.rs`); `decode` accepts only what `encode` produces. A host sees inside an iterator (`value::Source`), and only one that keeps what its source keeps (`Source::keeps`) is decoded or made in a program: the runtime steps without looking.
- `core::Tree::resolved` defines every hash: the tree's encoding with slots replaced by hashes and unions ordered by that. `Tree::encode` is the stored form, which names can order; never hash it. Changing either or the node set means bumping `FORMAT`.

## Testing

- Tests build a `Workspace` in memory and run an `Engine` over a `MemoryStore`; nothing touches disk, except `tests/disk.rs`, which tests the `DiskStore` under the system's temporary directory.
- Programs are built with builders. Text is for tests of the reader, of library methods (`support::text`), and of whole packages.
- A program is run with `support::run` or `support::run_with`, which run it natively and in wasmtime and fail if they disagree or if either leaks an object.
- An incremental test builds twice over one store and asserts on `Engine::ran` and `Engine::reused`.

## Committing

Keep all of these passing:

- `treefmt --ci` (`treefmt` to fix).
- `cargo clippy --workspace --all-targets --quiet`. Lints live in `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.
- `cargo nextest run --workspace`.
