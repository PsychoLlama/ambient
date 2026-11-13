# Iteration

An iterator is a function that gives the next item and an iterator of the rest. Everything that is iterated is iterated through one.

## Iterators

- An iterator of `T` is a [record](types.md) with a function that steps it: it takes nothing, and gives `null` when nothing is left, or else a tuple of an item and an iterator of what follows it.
- The item is in a tuple so that an item that is `null` is not the end.
- `Iterator<T>` is a record type of the standard library, not a [shape](values.md): any record of that type is an iterator.
- A record may hold its step as a [method](methods.md), since reading one gives that function.
- An iterator holds nothing but its step: what transforms and folds one is a function of the module that defines the type, called as a [method of the type](methods.md).
- One is made by writing the record, by a function of the standard library from a function that steps, or by merging over another.
- A record has those methods once its type is `Iterator<T>` by name, as what a function returns at that type is.
- Stepping changes nothing: the iterator that was stepped still gives the same step.
- An iterator is therefore a value like any record: it can be kept, stepped twice, compared, and [sent](exchange.md).
- Nothing is computed until an iterator is stepped, so one may be endless.
- What steps an iterator [captures](abilities.md) the abilities it performs, so an iterator that performs has the same type as one that does not.
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
- TODO: a list's iterator tells a member that is `null` from the end by testing it against the member type, so it holds its [type argument](values.md) and cannot be sent.
- TODO: a step allocates a tuple and an iterator, and collecting into a list [copies at every push](values.md); iterating a list is far dearer than indexing it until both are dealt with.
