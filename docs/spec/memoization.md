# Memoization

A memoized run caches a function's result together with everything the run observed. Comptime calls, [tools](tools.md), and tests are all memoized runs.

## Traces

- A memoized run is keyed by the hash of its [program](targets.md) and its arguments.
- The key therefore covers every handler written in Ambient that the entry reaches; only native code lies outside it.
- It stores the result and a trace: the [recording](handlers.md) of every ability performed.
- A trace is taken where a run meets [native code](externs.md), which is the only place it can observe anything its key does not cover.
- A run that failed is stored with its trace like any other, so a failure lasts only as long as what caused it.
- A stored run is reused only if every perform in its trace, repeated now, yields the same result.
- A run with an empty trace is valid for as long as its hash exists.
- Nothing is declared: no dependency lists, watch lists, or manual invalidation.
- A run that uses tasks and cells is still reproducible, because its target [schedules deterministically](tasks.md).

## Verifiable and volatile

- The [target](targets.md) a run executes under classifies each ability it provides as verifiable or volatile.
- A verifiable ability can be repeated cheaply and without side effects, and its results compare by digest.
- A trace that contains a volatile perform is never reused.
- The classification belongs to the handler, not to the function that performs.

## Tests

- A [test](testing.md) is a memoized run, so whether it runs again follows from its trace, not its row.
- A test answered wholly by handlers written in Ambient and by verifiable abilities is reused while its hash and those answers last, whatever its row names.
- A test that performs a volatile ability runs again on every run, with no annotation.
- A run with an empty trace is valid on any machine and can be shared between them.

## Dependents

- A definition whose content came from a [comptime](comptime.md) call that observed anything is stored with that call.
- It is reused only if the call, looked at again, still gives the same result.
- A call that is made again and gives what it gave before invalidates nothing.
