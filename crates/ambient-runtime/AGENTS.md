# ambient-runtime

What compiled code calls. The root `AGENTS.md` holds what spans crates; paths here are under `src/` unless they name another crate.

## Invariants

- A list is a view of a buffer that later lists share (`ambient-runtime/src/list.rs`); a push onto the list that ends its buffer writes into the room after it. Read a list's members through `list::members`, never `words`.
- Bytes are a shape of their own (`ambient-runtime/src/bytes.rs`), laid out as a string is and never tested as one; a host reads either through `amb_string_bytes`. One written out (`0x[89 50]`) is a `Literal::Bytes`: a constant the runtime makes from the block's text, as it does a string.
- An iterator is a shape of its own (`ambient-runtime/src/iterator.rs`): a function that steps, or a list or a range the runtime steps. `core::Expr::Step` asks for the function and calls it, or has the runtime advance (`Lowering::step`); everything else about iterators is library code over a step (`library/iterator.rs`).
- A handle is a shape of its own (`ambient-runtime/src/handle.rs`, `Value::Handle`): a brand's tag and a number that is its host's. A bare brand type is its type, tested by the tag (`descriptor::HANDLE`).
- The runtime queues each handle it reclaims, and a host drains the queue through `Waits::dropped` (`Program::heard`, `Guest::heard`): before native code runs, in the wait loop, and after a call. Both backends drain at the same moments, so a scripted world is left the same.
- A task that suspends beneath native code is resumed where the native code stands (`ambient-runtime/src/task/back.rs`): it is `pinned`, other tasks take turns there, and one that only the world could wake unwinds as `Fault::Pinned`. No lowering knows of it: an extern that does not answer later is never a point.
- A wait is written once in the runtime, as a step that says the task is suspending and is asked again on its turn (`task::waiting`). Whatever suspends goes through it; the scheduler never knows how a task is kept.
- The tasks waiting on a cell are a ring the cell names the first of (`ambient-runtime/src/task/watch.rs`), so a change costs only what waits on that cell. A task leaves the ring before it is cancelled and as each step of its wait begins, so a cell is never named by a task that is over.
- A task's number is its place in the scheduler, which the next task takes once it is over (`task/places.rs`); a join's is the same. Whatever names a task by number stops naming it in `Scheduler::finish`: the join that started it.
- Natively a task is a stack (`ambient-runtime/src/task/stack/`, `ambient-host/src/native/stacks.rs`): the host puts `Stacks` in the context, a wait parks inside its step, and a turn resumes the stack.
- A native handler declared `suspending` is called through `amb_native_wait` (`ambient-runtime/src/task/wait.rs`): native code takes a ticket (`Running::later`), the task suspends, and the host's loop takes answers from `Waits` when the scheduler leaves the mode `WAITING`. A task that leaves a wait tells the host at once, through the native entry with the index `FORSAKEN`.
- The runtime is `no_std`, and in wasm has its own allocator (`heap.rs`), unlocked state (`state.rs`), and a panic handler that traps. Use `core` and `alloc` there, never `std`, and no `format!` on a path every program reaches.
- A loaded block is a unit (`ambient-runtime/src/unit.rs`): it starts with a count, and a word naming the unit precedes each definition, fixed object, and constant descriptor in it. Closures, cells, type arguments, and dependent units hold the count; `Program::reclaim` drops units at zero.
- A native `Program` keeps a list of its live cells and empties them when it drops (`ambient-runtime/src/cell.rs`), since counting never reclaims a cell that reaches itself. In wasm the instance's memory goes as a whole.
