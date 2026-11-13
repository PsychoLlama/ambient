# Methods

A method is a function reached through a value it then receives. A record's methods are fields that receive the record they are read from; a type definition's are functions of its module, and a built-in shape's are functions of the standard library. Which one a call means is decided when it is checked, so nothing is looked up as a program runs.

## Calls

- A method call names a receiver, a method, and arguments.
- The static type of the receiver decides what it means: a field of a record, or else a method of the type definition it names, or else a method of its built-in shape.
- A receiver whose type is a union of different shapes, a type parameter, or `unknown` has no methods until it is [narrowed](types.md).
- A method call compiles to a call of one field or of one known definition; no table of methods can be changed as a program runs, and there is no chain to search.

## Record methods

- A field of a record may be a method: a function whose first parameter is the receiver.
- The receiver is the record the method is read from, always: reading the field gives a function already bound to that record.
- A method therefore cannot be detached from its record or called on another, and a receiver is never missing.
- Only a method sees the receiver: a field's value is evaluated before the record exists.
- A method is written in the record or is any function named there.
- A method says what it needs of its receiver as a type, and a record is checked against that where it is made.
- A method that asks for less than a record holds is a method of every record that has that much.
- A method written in a record that asks for nothing asks for that record as a reader sees it: each method as the function reading it gives.
- It reaches another such method through the receiver only if that one states what it takes and gives, as a [definition](definitions.md) reached by name does.
- One written in a record that is merged over another asks for the merged record.
- Calling a method makes no function, and reading one makes one; a record never holds a reference to itself.

## Method types

- A record type says of each key whether it is a method, and what the method needs of its receiver.
- A method written in a record type with no need stated needs that record type.
- A method is a subtype of one that needs more of its receiver, and of the function type that reading it gives.
- A field typed as that function is no longer a method to the checker: what it needed is forgotten.
- A [cast](types.md) never yields a method: one comes out of `unknown` as the function reading it gives.

## Merging

- A record merged over another, as a literal over what it spreads, has the methods of both, and each receives the merged record.
- Each method the operands' types list is checked against the merged record where the merge is written.
- A method therefore sees fields the merge replaced, and methods it replaced: a merge overrides.
- An override is visible where it is written and reaches only records made by that merge; the record merged over is unchanged.
- A method the checker cannot check stays bound to the record it had: one its type lists as a function, or does not list.
- A merge never gives a method a receiver it was not checked against.

## Keys

- A key is a [definition](definitions.md): a [tag](definitions.md), and the type of what a record holds under it.
- A record may hold a field or a method under a key as it does under a name, and its type says so.
- A record names a key where it names an [ability](handlers.md) it handles; a name that is both means the ability.
- Two keys are the same only if they are the same definition, so a key collides with no name and with no other package's key.
- A record holds under a key only a value of the key's type; holding another is a type error where the record is made.
- A key is how a protocol names what it expects of a record without reserving a name.

## Type methods

- A [type definition](types.md) has methods: the public functions of the module it is defined in whose first parameter is that type.
- A method call on a value whose type names the definition is a call of that function with the receiver first.
- The value does not hold such a method: it is not a field, so it is never read, merged, or overridden.
- Only a record holds methods of its own, and only as fields.
- A value has a type's methods only while its type has the name: a record written as a literal or made by a merge has none until it is given the type.
- What a pattern binds keeps the name its type had, unless the pattern narrowed it.
- A type's method comes before its shape's, so the module that defines a type decides what a call on its values means.
- Adding a function to a module changes only calls of that name on values of the module's own types.
- TODO: a type definition that is not recursive is [transparent](types.md), so its name is gone before a call is checked and it has no methods.

## Built-in methods

- Each built-in [shape](values.md) other than a record has methods: the functions of its module of the standard library whose first parameter is that shape.
- A method call on one is a call of that function with the receiver first.
- No other code adds a method to a shape; it writes a function, called as a function.
- A built-in method is called, never read as a field; the function is named in its module where a value is wanted.
- Adding a method to a shape changes no definition already checked, since each call holds the hash of what it meant.
- A [brand](types.md) marks a value and carries no methods.
