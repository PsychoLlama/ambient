# Cleanup

A cleanup is code registered in a block to run when that block exits, by any route. It is the only mechanism for releasing what a block acquired, and it travels with what was acquired.

## Registering

- A value carries its own cleanup: it is a record that holds a function under a [key](methods.md) of the standard library.
- A using binds such a value for the rest of its block and registers that function as a cleanup of the block.
- Whoever makes the value decides how it is released, and whoever binds it cannot forget to.
- A using may bind by a pattern. The value is released whole; the pattern only names its parts.
- There is no other way to register a cleanup: code that has only something to run binds a record that holds it.
- The function is read from the value when the using is evaluated, so what runs is fixed before the block does anything.

## Running

- A cleanup runs when its block exits by completing, returning, [aborting](handlers.md), being [cancelled](tasks.md), or [faulting](values.md).
- Cleanups in a block run in reverse order of registration.
- A cleanup may perform any ability in scope where it was registered.

## Completion

- A cleanup runs to completion: cancellation is not delivered inside one.
- An abort that leaves a cleanup is a fault.
- A fault inside a cleanup does not stop the cleanups still to run.
- A cleanup that fails while its block is unwinding does not change where the unwinding goes: the abort, cancellation, or fault that was under way continues, and the cleanup's own failure is dropped.
- A fault in the cleanup of a block that completed unwinds from that block.
- A cleanup that is still waiting when a program [stalls](tasks.md) is cancelled like any other code.

## Limits

- Release is tied to a block, never to reachability: no code runs because a value became unreachable.
- A call in a block with a registered cleanup is not in tail position.
