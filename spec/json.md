# JSON

The standard library reads and writes JSON. It is written in the language, over the [dictionary](dictionaries.md) and nothing of the runtime.

## Shape

- A JSON value is a union: null, a boolean, an integer, a float, a string, a list of values, or a dictionary of values.
- It is no shape of its own, so a value read is used as any other is: matched, cast, and passed to what takes a list or a dictionary.
- A number written with no fraction and no exponent is an integer if it fits one, and otherwise the nearest float.
- An object's members are read in the order of their names, and a name written twice holds what it was given last.

## Reading

- Reading takes the whole of a text: anything after the value but space is an error.
- Text that is not JSON [throws](errors.md) an error of one kind, whose message says what was expected and at which line and column.
- Reading is strict: no comments, no trailing commas, no numbers JSON does not write.
- An escape that names half of a character with no other half is an error, so every string read is [valid](operations.md).

## Writing

- Writing puts nothing between the parts of a value, and escapes only what JSON cannot write as itself.
- What is written reads back as the same value.
- A float that is no number is written as null, since JSON has no way to say it.

## Reaching in

- A member is reached by name and an item by position; reaching into a value that is not an object or an array gives null, as reaching for what is absent does.
- TODO: an object cannot be [tested](types.md) for by its type, since a dictionary holds methods; a match tells it from the rest as the record it is, or by being what is left.
