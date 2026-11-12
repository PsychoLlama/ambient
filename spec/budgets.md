# Budgets

A budget bounds how much work and allocation a body may do, and enforces the bound deterministically. Code pays for metering only while it runs inside a budget.

## Expression

- A budget expression takes limits and a body, and yields either the body's value or the limit that was exhausted.
- The limits are a number of steps and an amount of allocation.
- Exhaustion unwinds the body as [cancellation](tasks.md) does: cleanups run, and the body cannot observe or intercept it.
- Exhaustion is neither a [fault](values.md) nor an ability.
- A charge inside a nested budget counts against every budget enclosing it.
- The limit a budget yields is a [tag](definitions.md) of the language's own package.
- An operation is charged before it happens, so a budget that cannot afford one never performs it.
- TODO: a cleanup that runs because its budget ran out is charged against that budget too, so it ends at its first charge and can release nothing.

## Charges

- A step is charged for each call; loops are calls, so no computation is free.
- Performing an ability, starting a task, and running a cleanup are calls.
- Allocation is charged for each value constructed, by its size.
- Charges are defined on the [core tree](trees.md), never on compiled code, so a run costs the same on every backend and machine.
- A node is charged for before any of it is evaluated, by what the node is alone; what a backend or the runtime allocates to carry it out is not charged.
- The allocation charge is cumulative and ignores sharing and in-place update; it bounds peak memory from above.
- Tasks started inside a budget draw from it together.

## Scope

- Everything that runs within the body is metered, including the functions it calls and the tasks it starts.
- A handler bound outside the budget is not metered; performing one charges a fixed number of steps.
- What a host provides to budgeted code therefore costs that code a predictable amount, whatever the host does to serve it.

## Cost

- A definition is compiled in metered form only if it runs inside a budget; all other code is unchanged and pays nothing.
- The metered and unmetered forms of a definition have the same hash.
- Metered code can be cancelled at any charge, so a task inside a budget can always be cancelled.
