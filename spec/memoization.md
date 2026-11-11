# Memoization

A memoized run caches a function's result together with everything the run observed. Comptime calls and tests are both memoized runs.

## Traces

- A memoized run is keyed by the function's hash and its arguments.
- It stores the result and a trace: the [recording](handlers.md) of every ability performed.
- A stored run is reused only if every perform in its trace, repeated now, yields the same result.
- A run with an empty trace is valid for as long as its hash exists.
- Nothing is declared: no dependency lists, watch lists, or manual invalidation.

## Verifiable and volatile

- The [target](targets.md) a run executes under classifies each ability it provides as verifiable or volatile.
- A verifiable ability can be repeated cheaply and without side effects, and its results compare by digest.
- A trace that contains a volatile perform is never reused.
- The classification belongs to the handler, not to the function that performs.

## Tests

- A test is a function run under a test target; it passes if it returns.
- A test with an empty row reruns only when its hash changes.
- A test that needs real effects performs volatile abilities and always reruns, with no annotation.
- A run with an empty trace is valid on any machine and can be shared between them.
