# Methods

A method is a function reached through a value it then receives. A record's methods are fields bound to the record when it is made; the methods of any other value are functions of the module that owns its type. Which one a call means is decided when it is checked, so nothing is looked up as a program runs.

## Calls

- A method call names a receiver, a method, and arguments.
- The static type of the receiver decides what it means: a field of a record, or a method of a nominal type.
- A receiver whose type is a union of different shapes, a type parameter, or `unknown` has no methods until it is [narrowed](types.md).
- A receiver that is both a record with that field and a brand with that method is an error, not a preference.
- A method call compiles to a call of one known definition or of one field; no table of methods exists at run time, so none can be changed.

## Record methods

- A field of a record literal may be a method: a function whose first parameter, the receiver, is the record being made.
- The receiver is that literal's record, whole, with whatever it spreads; it is never anything else, however the method is later reached.
- The receiver's type is written or is the literal's own, and the literal is checked against it where it stands.
- A method is written in the literal or is any function named there; a function that asks for less than the record holds is a method of every record that has that much.
- The field's type is the function's without its first parameter, so reading the field gives a function that needs no receiver.
- Only a method sees the receiver: a field's value is evaluated before the record exists.
- A method reached through the receiver states its return type, as a [definition](definitions.md) reached by name does.
- A literal that spreads a record has that record's methods as it has its fields, still bound to the record they were made in.
- A method therefore never sees a field changed after it was bound; a record whose methods see new fields is made by making the record again.
- A method's own record is never what keeps the record alive: calling a method makes no function, and reading one makes one function.

## Keys

- A key is a [definition](definitions.md): a [tag](definitions.md), and the type of what a record holds under it.
- A record may hold a field under a key as it does under a name, and its type says so.
- Two keys are the same only if they are the same definition, so a key collides with no name and with no other package's key.
- A record holds under a key only a value of the key's type; holding another is a type error where the record is made.
- A key is how a protocol names what it expects of a record without reserving a name.

## Nominal methods

- A nominal type is a built-in [shape](values.md) other than a record, or a [brand](types.md).
- Its methods are the functions of the module that owns it whose first parameter is that type: for a built-in shape a module of the standard library, for a brand the module that declares it.
- A method call on a nominal type is a call of that function with the receiver first.
- Only the owning module adds methods, so no two packages can disagree about what a method of a type is.
- Code elsewhere extends a type with functions of its own, called as functions.
- A nominal method is called, never read as a field; the function is named in its module where a value is wanted.
- A brand's methods are not fields: a branded value meets a record type only through a record written to hold them.
- Adding a method to a type changes no definition already checked, since each call holds the hash of what it meant.
