# Cleanup

A cleanup is code registered in a block to run when that block exits, by any route. It is the only mechanism for releasing what a block acquired, and it is written next to the acquisition.

## Running

- A cleanup runs when its block exits by completing, returning, [aborting](handlers.md), being [cancelled](tasks.md), or [faulting](values.md).
- Cleanups in a block run in reverse order of registration.
- A cleanup may perform any ability in scope where it was registered.

## Completion

- A cleanup runs to completion: cancellation is not delivered inside one.
- An abort that leaves a cleanup is a fault.
- A fault inside a cleanup does not stop the cleanups still to run.

## Limits

- Release is tied to a block, never to reachability: no code runs because a value became unreachable.
- A call in a block with a registered cleanup is not in tail position.
