# Maps

A map holds values by key, and its type says which type of value each type of key holds. It is a type of the standard library written in the language, not a shape of the runtime.

## Shape

- A map is an [opaque type](types.md#opaque-types) whose [methods](methods.md) are its operations, so it is used as a list is: `prices.get("tea")`.
- What holds its entries is seen only by its own module, so no other code reads it, builds one, or changes what an operation does.
- It is made empty, or of a list of entries.
- It is a value like any other: setting or removing a key gives another map and leaves the first as it was.

## Entries

- A map's type is that of its entries: a union of pairs, each a key type with a value type, as `Map<(key: "name", value: string) | (key: "age", value: int)>`.
- The relation between a key's type and its value's is kept: no reading of a map flattens it into two independent unions.
- Reading a key gives the value types of every pair whose key type the key's [may match](types.md#type-matches), or null when the key is absent. The standard library says so with a type match, so a map holds no rule of the checker's.
- A key whose type matches one pair gives that pair's value type; a key that may match several gives the union of theirs.
- Narrowing therefore needs key types that are [disjoint](types.md#overlap): literals and tags are, and two brands on one type are not.
- Reading a key's entry gives the pair, of the pairs its type may match, or null: a value that is null is then told from none in one reading.
- Null for absence stays the common reading, since most values are never null; no reading wraps what it gives in a type of its own.
- Setting takes an entry, which must be of the map's entry type.
- An update gives a map with the entry a function makes of the value under a key, or of null, set as setting would: counting is one call.
- An update's function gives an entry and not a value. A key that may match several pairs reads a union of their values, and a value of that union under the key would not be an entry.
- A map made of a list of literal entries holds their keys widened to their primitives, as any [inferred](types.md#generics) type argument a value is written to is; a map that narrows has its entry type written out.

## Keys

- A key is null, a bool, an int, a string, bytes, or a tag.
- Keys, values, and entries are read in the [order](operations.md#order) of the keys, never the order they were set in.
- Two maps with the same entries read the same, so nothing a program [observes](memoization.md) depends on how one was built.
- A key of any other shape waits for the order to take it.

## Cost

- Reading, setting, and removing take time in the logarithm of the size, and a changed map shares all but one path with the one it was made from.
- Reading a key tests the entry it finds against the key's type, which is how the reading is checked; it is [handed](backends.md) the key's type to test against.

## Sets

- A set holds keys, each once, and is an opaque type of the standard library: `Set<T>`, with `T` a key type.
- It is not a map: what holds its keys is its own module's, whatever that is.
- It is made empty, or of a list of keys, and is a value as a map is: adding or removing a key gives another set.
- Its keys are read in their order, so two sets of the same keys read the same.
- Two sets combine into the keys of either, of both, or of the first and not the second.
