# Iteration

An iterator is a function that gives the next item and an iterator of the rest. Everything that is iterated is iterated through one.

## Iterators

- An iterator of `T` is a function that takes nothing and gives `null` when nothing is left, or else a [tuple](types.md) of an item and an iterator of what follows it.
- The item is in a tuple so that an item that is `null` is not the end.
- `Iterator<T>` is that function type under a [brand](types.md) of the standard library, not a [shape](values.md); a function of the standard library makes an iterator of any function of that type.
- Being a brand is what gives an iterator [methods](methods.md).
- Stepping an iterator is calling it. It changes nothing: the iterator it was called on still gives the same step.
- An iterator is therefore a value like any function: it can be kept, stepped twice, compared, and [sent](exchange.md).
- Nothing is computed until an iterator is stepped, so one may be endless.
- An iterator's function [captures](abilities.md) the abilities it performs, so an iterator that performs has the same type as one that does not.
- Stepping an iterator that performs performs again; what it gives is then whatever its handlers answer.
- An iterator over something a [cleanup](cleanup.md) releases is good only inside the block that registered the cleanup.

## Sources

- A collection is never itself an iterator; a function of the standard library gives an iterator over it.
- A collection that can be walked more than one way has a function for each.
- A range of integers is an iterator, and holds no list.

## Use

- A loop over an iterator is a function that steps it and calls itself in [tail position](backends.md) with the rest, so it runs in constant stack.
- The iterator's methods transform iterators into iterators without stepping them, and fold one into a value by stepping it to its end.
- No [operation](operations.md) takes an iterator, since an operation calls nothing.
- Any construct the language gains for looping takes an iterator and nothing else.
- TODO: a step allocates a tuple and a function, and collecting into a list [copies at every push](values.md); iterating a list is far dearer than indexing it until both are dealt with.
