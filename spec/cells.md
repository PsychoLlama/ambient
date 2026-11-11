# Cells

A cell is the only mutable thing in the language: a handle to a slot that holds one immutable value. Everything else that looks like mutation is built from cells or is an in-place update no one can observe.

## Operations

- Creating a cell takes its initial value.
- Reading a cell yields the value it holds.
- Replacing takes the value the caller read and a new value, succeeds only if the cell still holds the value read, and reports which happened.
- Waiting takes the value the caller read and suspends the [task](tasks.md) until the cell holds a different one.
- There are no other operations; updates, locks, queues, and channels are library code over these four.
- Cell operations are not abilities: holding the cell is the authority to use it.

## Guarantees

- A read sees a whole value, never part of one.
- A failed replace changes nothing.
- No operation spans two cells; state that must change together lives in one cell.
- A cell is [anchored](values.md).
- A cell records the type it was created at, so a cast to a cell type is decidable.

## Determinism

- A definition with an empty row that neither receives nor returns an anchored value cannot expose a cell, so the cells it creates are unobservable.
- Such a definition is deterministic unless it shares a cell between tasks, and then only under a target that schedules in parallel.

## Speed

- Mutation for speed does not need a cell; see [sharing](values.md).
