# Tasks

A task is an independent execution of an expression. Tasks exist only inside the construct that started them, so no task outlives its starter, no failure goes unobserved, and nothing in this page can fault.

## Groups

- A group is a block; a spawn written in its body starts a task in that group.
- A spawn belongs to the group it is written in: it appears lexically in the group's body, never inside a nested function.
- A group completes when its body has finished and every task spawned in it has finished; its value is the body's.
- A spawned task yields no value and no handle; nothing can refer to a running task.
- A callee cannot add tasks to its caller's group; code that needs tasks opens its own group.
- A spawn may use any handler bound outside its group, and none bound inside it, since that provide could exit first.
- A spawn under a [recording](handlers.md) or an only-mode provide written inside its group may use no handler at all, since either stands between it and what is bound outside.
- A task that fails disturbs its group's body: the body unwinds from its current or next suspension as if cancelled, and the group then unwinds as the task did.
- A program's entry runs as its root task.

## Joining

- An all expression evaluates a fixed set of two or more expressions as tasks and yields the tuple of their results.
- A first expression evaluates a fixed set of expressions as tasks, yields the result of whichever finishes first, and cancels the rest.
- Both complete only after every branch has finished or unwound.
- A branch is evaluated with the locals and [bindings](abilities.md) of where it is written.
- Each also takes a list of functions that take nothing and perform nothing, and runs as many tasks as the list holds: an all gives the list of their results in order, and a first the result of the first.
- Those are [operations](operations.md) the standard library wraps, so how many tasks there are need not be known where the code is written. A function in the list is a closure: its task has the locals and bindings of where the function was made.
- A first over an empty list would wait for ever; the library gives null instead.
- A task's result reaches other code through one of these two forms or through a [cell](cells.md); there is nothing to await.

## Sharing

- Tasks share values freely, since values are immutable; the only mutable thing two tasks can share is a cell.
- Tasks that share no cell cannot race: their results do not depend on scheduling.

## Suspension

- A task suspends only when it waits on a group, an all, a first, or a cell, or performs an ability whose [native handler](externs.md) suspends.
- A task cannot suspend beneath [native code](externs.md#calling-back) that is calling back into the program.
- Suspension is invisible in types: no function is marked as suspending, and any function may call any other.
- There is one way to write code, and it reads as blocking.

## Scheduling

- How tasks are scheduled belongs to the [target](targets.md), not the program.
- A target may run tasks in parallel, or interleave them on one thread, switching only at suspensions.
- A program is correct under either; it may not assume that code between two suspensions runs atomically.
- A target that owns no thread, such as a browser or an embedding host, runs tasks only while its host calls in, and returns once every task is suspended.
- No task ever blocks a host's thread.
- A task waiting on a [native handler](externs.md) can be woken from outside: while one waits, the target waits for the host's answer.
- When every task of a program is suspended and nothing outside it can wake one, its root task is cancelled, so the program ends by unwinding instead of hanging.
- Build and test targets schedule deterministically, so a run that mixes tasks and cells is reproducible.

## Failure

- An [abort](handlers.md) raised in a task leaves that task, cancels the others started by the same construct, and continues unwinding from that construct.
- A [fault](values.md) in a task does the same, and continues as a fault in the task that started it.
- An abort or fault that leaves a group's body cancels the group's tasks.
- A construct finishes unwinding only after every one of its tasks has.
- A failure is therefore never lost and a task is never orphaned.

## Cancellation

- A task is cancelled only because the construct that started it is unwinding or because it lost a first.
- A cancelled task unwinds from its current or next suspension, running its [cleanups](cleanup.md).
- A task cannot observe, intercept, or refuse cancellation, and no code can wait on a cancelled task.
- Outside a [budget](budgets.md), a task that never suspends cannot be cancelled; its starter waits for it.
