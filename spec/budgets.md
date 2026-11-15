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
- TODO: metering gives every function that can run in a budget a second compiled form and every operation a charge to define, and code that needs a budget is rare; whether budgets earn that is open.
- A budget that ran out allows a small fixed amount more, which only the cleanups it unwinds through draw on, so they can release what they hold.
- A cleanup that needs more than that ends as the body did, so a budget still bounds everything inside it.

## Charges

- A step is charged for each call; loops are calls, so no computation is free.
- Performing an ability, starting a task, and running a cleanup are calls.
- Allocation is charged for each value constructed, by its size.
- Charges are defined on the [core tree](trees.md), never on compiled code, so a run costs the same on every backend and machine.
- A node is charged for before any of it is evaluated, by what the node is alone; what a backend or the runtime allocates to carry it out is not charged.
- The allocation charge is cumulative and ignores sharing and in-place update; it bounds from above the memory held by the values a body makes.
- It does not bound what a backend or the runtime allocates to carry the body out, such as the saved state of a suspended task.
- Tasks started inside a budget draw from it together.

## Scope

- Everything that runs within the body is metered, including the functions it calls and the tasks it starts.
- A handler bound outside the budget is not metered; performing one charges a fixed number of steps.
- What a host provides to budgeted code therefore costs that code a predictable amount, whatever the host does to serve it.

## Cost

- A definition is compiled in metered form only if it runs inside a budget; all other code is unchanged and pays nothing.
- The metered and unmetered forms of a definition have the same hash.
- A definition [loaded](exchange.md) into a running program has both forms, since where it will be called from is not known.
- Metered code can be cancelled at any charge, so a task inside a budget can always be cancelled.
