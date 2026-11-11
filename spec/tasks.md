# Tasks

A task is an independent execution of a function value. Tasks exist only inside groups, so no task outlives the code that started it and no failure goes unobserved.

## Groups

- A group expression runs a body and passes it a group; a task is spawned into a group and nowhere else.
- A group expression completes when its body has returned and every task spawned into it has finished; its value is the body's.
- A group is a [handle](values.md): passing it to a callee lets the callee spawn into it, and nothing else does.
- Spawning into a completed group is a [fault](values.md).
- A program's entry runs as its root task.

## Tasks

- Spawning takes a function value and returns a task; what the function [captured](abilities.md) goes with it.
- Awaiting a task suspends the awaiter until the task finishes and yields its result.
- Tasks share values freely, since values are immutable; the only mutable thing two tasks can share is a [cell](cells.md).
- Tasks that share no cell cannot race: their results do not depend on scheduling.

## Suspension

- A task suspends only when it awaits a task, waits on a cell, or performs an ability whose [native handler](externs.md) suspends.
- Suspension is invisible in types: no function is marked as suspending, and any function may call any other.
- There is one way to write code, and it reads as blocking.

## Scheduling

- How tasks are scheduled belongs to the [target](targets.md), not the program.
- A target may run tasks in parallel, or interleave them on one thread, switching only at suspensions.
- A program is correct under either; it may not assume that code between two suspensions runs atomically.
- A target that owns no thread, such as a browser or an embedding host, runs tasks only while its host calls in, and returns once every task is suspended.
- No task ever blocks a host's thread.
- Build and test targets schedule deterministically, so a run that mixes tasks and cells is reproducible.

## Failure

- An [abort](handlers.md) whose provide encloses a group leaves the task that raised it, cancels the rest of the group, and continues unwinding from the group expression.
- A fault in a task does the same, and continues as a fault in the task that owns the group.
- An abort or fault that leaves the body cancels the group's tasks.
- A group expression finishes unwinding only after every one of its tasks has.
- A failure is therefore never lost and a task is never orphaned.

## Cancellation

- Cancelling a group cancels every unfinished task in it; there is no other way to cancel a task.
- A cancelled task unwinds from its current or next suspension, running its [cleanups](cleanup.md).
- A task cannot observe, intercept, or refuse cancellation.
- A task that never suspends cannot be cancelled; its group waits for it.
- Awaiting a cancelled task is a fault.
