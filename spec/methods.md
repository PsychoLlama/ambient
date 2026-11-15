# Methods

A method is a function reached through a value it then receives. A record's methods are fields that receive the record they are read from; a built-in shape's methods are functions of the standard library. Which one a call means is decided when it is checked, so nothing is looked up as a program runs.

## Calls

- A method call names a receiver, a method, and arguments.
- The static type of the receiver decides what it means: a field of a record, or a method of a built-in shape.
- A value is a record or it is not, so a call has one meaning and nothing is tried in turn.
- A record has the methods written in it and no others; a type definition, a module, and a name in scope add none.
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
- A method may take [type parameters](types.md): calling it gives them by its arguments, and reading it gives a generic function bound to the record.
- What a generic method needs of its receiver mentions none of its type parameters: a record is held to it where it is made, before any call has chosen them.
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

- A key is a [definition](definitions.md) with an [identity](definitions.md), and the type of what a record holds under it.
- A record may hold a field or a method under a key as it does under a name, and its type says so.
- A record names a key where it names an [ability](handlers.md) it handles; a name that is both means the ability.
- Two keys are the same only if they are the same definition, so a key collides with no name and with no other package's key.
- A record holds under a key only a value of the key's type; holding another is a type error where the record is made.
- A key is how a protocol names what it expects of a record without reserving a name.
- Only a record holds a key: what the other [shapes](values.md) do is the language's to say, so no protocol reaches them.
- A key has no fallback: a record that does not hold one does not meet the protocol, and reading it there is a type error.

## Built-in methods

- Each built-in [shape](values.md) other than a record has methods: the functions of its module of the standard library whose first parameter is that shape.
- A method call on one is a call of that function with the receiver first.
- No other code adds a method to a shape; it writes a function, called as a function.
- A built-in method is called, never read as a field; the function is named in its module where a value is wanted.
- Adding a method to a shape changes no definition already checked, since each call holds the hash of what it meant.
- A [brand](types.md) marks a value and carries no methods.
