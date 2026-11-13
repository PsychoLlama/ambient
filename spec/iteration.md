# Iteration

An iterator is a value that gives the next item and an iterator of the rest. It is a shape of its own, and everything that is iterated is iterated through one.

## Iterators

- An iterator is a built-in [shape](values.md), and `Iterator<T>` is the type of one whose items are of type `T`.
- Stepping one gives `null` when nothing is left, or else a tuple of an item and an iterator of what follows it.
- The item is in a tuple so that an item that is `null` is not the end.
- An iterator is made from a function that steps: one that takes nothing and gives what a step gives.
- The language makes the iterators over its own collections and over a range of integers; stepping one of those calls nothing.
- A record is never an iterator; one that can be iterated has a field that gives an iterator.
- Stepping changes nothing: the iterator that was stepped still gives the same step.
- An iterator is therefore a value like any other: it can be kept, stepped twice, and compared.
- Nothing is computed until an iterator is stepped, so one may be endless.
- What steps an iterator [captures](abilities.md) the abilities it performs, so an iterator that performs has the same type as one that does not.
- Stepping an iterator that performs performs again; what it gives is then whatever its handlers answer.
- An iterator over something a [cleanup](cleanup.md) releases is good only inside the block that registered the cleanup.
- `Iterator<T>` is covariant in `T`.

## Casts

- A [cast](types.md) to an iterator type tests an iterator the language made by the items it has left.
- It tests one made from a function as it tests that function: the function's type is exactly that of a function stepping such an iterator, or the test fails.
- An iterator type that mentions a type parameter is not a cast target.
- TODO: an iterator made inside a generic function steps by a function whose type depends on type arguments, so no cast passes it.
- TODO: an iterator has no [encoding](exchange.md) yet, so one cannot be sent.

## Sources

- A collection is never itself an iterator; a [method](methods.md) of it gives an iterator over it.
- A collection that can be walked more than one way has a function for each.
- A range of integers is an iterator, and holds no list.
- A string is walked by its [characters](operations.md), in order.

## Use

- A loop over an iterator is a function that steps it and calls itself in [tail position](backends.md) with the rest, so it runs in constant stack.
- An iterator's [methods](methods.md) transform iterators into iterators without stepping them, and fold one into a value by stepping it to its end.
- Those are functions of the standard library written over a step; only making an iterator and stepping one are the language's.
- A step is not an [operation](operations.md), since it may call the function that steps.
- Any construct the language gains for looping takes an iterator and nothing else.
- TODO: a step allocates a tuple and an iterator, so iterating a list is dearer than indexing it.
