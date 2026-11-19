# Maps

A map holds values by string key. It is a type of the standard library written in the language, not a shape of the runtime.

## Shape

- A map is a record whose [methods](methods.md) are its operations, so it is used as a list is: `prices.get("tea")`.
- It is made empty, or of a list of keys paired with values.
- It is a value like any other: setting or removing a key gives another map and leaves the first as it was.
- Reading a key that is absent gives null, as reading past the end of a list does.
- Reading a key's entry gives the key with its value, or null: a value that is null is then told from none in one reading.
- Null for absence stays the common reading, since most values are never null; no reading wraps what it gives in a type of its own.
- An update gives a map whose value under a key is made from the one there, or from null: counting is one call.

## Order

- Keys, values, and entries are read in the order of the keys, never the order they were set in.
- Two maps with the same entries read the same, so nothing a program [observes](memoization.md) depends on how one was built.

## Cost

- Reading, setting, and removing take time in the logarithm of the size, and a changed map shares all but one path with the one it was made from.

## Limits

- A key is a string. Other keys wait for a way to order any value.
