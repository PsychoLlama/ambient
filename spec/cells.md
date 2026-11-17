# Cells

A cell is the only mutable thing in the language: a slot that holds one immutable value. Everything else that looks like mutation is built from cells or is an in-place update no one can observe.

## Operations

- Creating a cell takes its initial value.
- Reading a cell yields the value it holds.
- Replacing takes the value the caller read and a new value, succeeds only if the cell still holds the value read, and reports which happened.
- Waiting takes the value the caller read and suspends the [task](tasks.md) until the cell holds a different one.
- A change gives the tasks waiting on the cell their turns in the order they began to wait.
- There are no other operations; updates, locks, queues, and channels are library code over these four.
- No cell operation can fault.

## Access

- A cell is a value: it is shared by passing it or capturing it, and every holder sees the same slot.
- Any holder may read, replace, and wait; a cell has no owner and no per-holder permission.
- Code that never receives a cell, directly or inside a function value, cannot touch it.
- Narrower access is granted by handing out a function that closes over the cell instead of the cell itself.
- Cell operations are not abilities; a cell is authority over itself and nothing else.
- A cell never leaves its program, because it is [anchored](values.md).

## Guarantees

- A read sees a whole value, never part of one.
- A replace is atomic: of several racing on one value read, exactly one succeeds.
- A failed replace changes nothing, so an update is a read, a computation, and a replace, retried until it succeeds.
- No operation spans two cells; state that must change together lives in one cell.
- A cell records the type it was created at, so a cast to a cell type is decidable: it passes a cell made at the same type.

## Determinism

- A definition with an empty row that neither receives nor returns an anchored value cannot expose a cell, so the cells it creates are unobservable.
- Such a definition is deterministic unless it shares a cell between tasks, and then only under a target that schedules in parallel.

## Speed

- Mutation for speed does not need a cell; see [sharing](values.md).
