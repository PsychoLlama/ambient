# Syntax

Source text is braces and statements, read by anyone who reads TypeScript, Rust, or Swift. Every form here is a way of writing a node of the [tree as written](trees.md); where a form is shorthand, this page says what it stands for.

## Statements

- A statement ends in `;`, and nothing is inserted where one is missing.
- A block is statements between braces and never has a value.
- `return` gives a body its value and is always written; a body that ends without one gives `null`.
- A body is a block, or `=>` and one expression, wherever a body is written.
- A brace where a value is expected is therefore always a record.
- `let` binds a name, or the names of a tuple or record pattern, for the rest of its block.
- An expression may stand as a statement; its value is dropped.
- A statement of type `never` ends its path as a `return` does: nothing after it is reached, and the path gives no value of its own.
- `for`, a pattern, `in`, a value, and a block runs the block for each item the value gives: it is the value's `each` [method](methods.md) called with a function of the block.
- A `for` is therefore never left early, and no `return` is written in its block; looking for an item is a method of the [iterator](iteration.md).

## Returning early

- The language has no node for leaving a function early, so a `return` is always the last thing some path through its body does.
- An `if` or a match written as a statement may return in some branches and not others; the statements after it are then what the other branches go on to do.
- In an arm, parentheses around members written `name: Type` are a tuple type the value is tested as, as after `let at:`; around anything else they take a tuple apart.
- A `let` may be followed by `else` and arms: the value is matched, the rest of the block is what happens where it fits the pattern, and each arm is what happens where it does not. Every such arm ends in a `return`.
- A name bound by a `let` may say what it binds, as `items: List<int>`; a value that may not be that needs an `else`.
- A `let` of one name that says what it binds, with no `else`, tests nothing: the value is held to that type, takes from it what it leaves out, and is bound as it.
- The body of a provide is a body like a function's: `return` gives the provide its value. The body of any other construct is a function.
- Nothing returns across one of those, since nothing but an [abort](handlers.md) leaves one early.

## Definitions

- A definition is written with the keyword of its [kind](definitions.md): `fn`, `type`, `ability`, `contract`, `tag`, `extern`, `target`, `const`.
- Only `pub` comes before the keyword. Anything else said of a definition's kind is a word in parentheses after the keyword, as `type(nominal)` is.
- A native handler that may suspend is written `extern handler(suspending)`.
- A native handler is written with its ability alone, or with a name, `for`, and its ability. `in` and targets, after its row, says where it is a [default](targets.md#default-bindings).
- An extern says what it performs as a function does: `with` and a row, after what it gives or the ability it handles.
- A word that only begins a definition is not reserved: `contract`, `tag`, `ability`, `target`, `extern`, `const`, `use`, and `mod` may name anything, since nothing else begins where a definition does.
- `pub` before one makes it public, and a `///` comment before one is its doc comment.
- An [annotation](annotations.md) is written before a definition, after its doc comment: `#[`, the key, the value in parentheses, and `]`. A definition may have several.
- An annotation written with no value holds `null`.
- The brackets close an annotation wherever one is written, so one before an expression needs no other mark.
- No definition is given a string to be known by: [identity](definitions.md) is read from where it is declared.
- An import is `use` and a path, or a path and a group in braces of what is taken under it; `as` and a name after one takes it under that name.
- A group nests, and `self` in one is the path the group is under.
- `pub use` re-exports, and `pub mod` in a library module lists a [public module](packages.md).
- Type parameters follow a name in angle brackets, each with an optional bound after a colon.
- Type parameters follow a definition's name in angle brackets, each a name and, after a colon, its bound if it has one: `type Map<E: Entry<Key, unknown>>`, `fn sum<T: int>`. A bound may name any of them.
- A function's row follows its return type after `with`: abilities, and modules for all of theirs.
- A [constant](constants.md) is `const`, its name, a colon and its type, `=`, and its value.
- A constant's name is capital letters, digits, and `_`, and begins with a letter; the reader refuses any other. No other definition's name is held to a case. A name of that form before a `.` is a value, as a local is, so what follows is a field of it; a module imported under such a name is not reached through it.

## Expressions

- A number written with a fraction or an exponent is a float, and any other is an integer; `_` may stand between digits.
- Parentheses around one expression, type, or pattern group it; around two or more they make a tuple.
- A call is a callee and arguments in parentheses; a method call puts a receiver and a dot before it.
- A [perform](abilities.md) is written as a call with `!` after the ability, so every place a function reaches outside itself is visible where it does.
- A lambda is `fn`, parameters, and a body.
- A function named in a block is a local that holds it, and its body may call it by that name; one that does states its types, as a [method](methods.md) does.
- A record is members between braces: a field, a field named for a local of the same name, a function or method written with its parameters, or `...` and a record to [merge](methods.md) over.
- A member whose first parameter is `self` is a [method](methods.md); with any other first parameter it is a function.
- A record names a [contract](methods.md) or an [ability](handlers.md) in square brackets, where it would name a field, and indexing by one in square brackets reads it.
- `${` and `}` in a string hold a value, and the string is its pieces [joined](operations.md) with each value's `to_text` [method](methods.md) called; `\$` writes the sign itself.
- A condition, `?`, a value, `:`, and another value is a match of the condition that gives the first where it is true.
- A parameter of a function written in place may be a pattern: the argument is matched against it where the body begins.
- A [cast](types.md) is a call of the [intrinsic](definitions.md#intrinsics) `cast` with its type written out, a value, and a fallback. It gives the value at that type, or the fallback.
- A cast's fallback is an argument like any other: it is evaluated after the value and before the test, whether the value passes or not.
- A value, `??`, and another value is a match of the first: the first where it is not `null`, and the second where it is, evaluated only then.
- `??` binds looser than every operator and tighter than `?`. A first value that is never `null` is an error, as an arm that is never taken is.
- A [comptime](comptime.md) call is `comptime` before the call.
- A derived type is a type definition with a comptime call where its type would be.
- A generated function is `fn`, its name, `=`, and a comptime call.
- A generated module is a file of imports and one comptime call, and nothing else.
- Type arguments are written out after `::` in angle brackets.
- An [operation](operations.md) is a call of the [intrinsic](definitions.md#intrinsics) of its name, whether or not it has an operator too. The standard library wraps each that has none in a function, and other code calls that.
- Operators are those of [operations](operations.md) and, built from them, the other comparisons, `&&`, `||`, and `!`; the last three evaluate only the operands they need.
- A brand is declared `type(nominal) Name;`, a type and a brand are intersected with `&`, which binds tighter than `|`, and `Name(value)` brands a value.
- Bytes are written `0x[` and `]` around pairs of hexadecimal digits, a byte each, with any space between pairs: `0x[89 50 4E 47]`, and `0x[]` for none.
- `+` between two strings, two lists, or two bytes values joins them, as between two numbers it adds; nothing else is joined, and nothing converts.
- `-` before a value is a call of its `negate` [method](methods.md), which the standard library gives the numbers; before a number written out it is part of the literal.
- `**` raises to a power. It binds tighter than `*`, `/`, and `%`, and groups to the right: `2 ** 3 ** 2` is `2 ** 9`.
- A `-` before the left operand of `**` is refused, number or not: `-2 ** 2` could be `(-2) ** 2` or `-(2 ** 2)`, so either is written with its parentheses.
- `if` is a statement, and a match is the conditional that has a value.
- A condition is matched like any other subject, so one whose type is a single literal has a branch that can never be taken, which is an error.

## Patterns

- A match used as a value has an expression for each arm, and one used as a statement has a block for each.
- A bare name in a pattern is a type or a [tag](definitions.md), never a binder, so a misspelt name is an error and not an arm that matches everything.
- `let` and a name binds what an arm matched; a colon and a pattern after it narrows what is bound.
- A pattern after `let` in a statement binds its bare names, since nothing there is tested.

## Scopes

- A [provide](handlers.md) is `provide`, a bundle, and a body; `provide(restricted)` is a restricted one.
- `restricted` is reserved, so those parentheses are never a bundle of that name.
- `using` and a binding registers a [cleanup](cleanup.md).
- Every other construct that takes a body is a call of an [intrinsic](definitions.md#intrinsics): `contain` for a [boundary](values.md), `observe` for a [recording](handlers.md), and `group`, `spawn`, `all`, and `first` of the standard library's `tasks` module for [tasks](tasks.md).
- Each takes its body, or each of its branches, as a function of no parameters, so nothing written in one reads as running in order with what is around it. A recording takes its observer before its body.
- A [cell](cells.md) is made by a call of the intrinsic `cell`, which holds what its first value is, as widely as a literal is read, unless a type is written out for it.

## Types

- A record type is written as a record is, with a type where a value would be.
- A member written with parameters whose first is `self` is a method type; a type after `self` is what it needs of its receiver.
- A function type is `fn`, parameters, a return type, and a row after `with`.
- A function type that is what a function gives is written in parentheses, as `fn(): (fn(): null) with log`: a row after one written bare could be either's.
- A parameter in a type and a member of a tuple type are each a name, a colon, and a type, as `(name: string, count: int)`. The name is for a reader: it is no part of the type, and a tuple value is written without one.
- A type is shown as it would be written, names and all. One that nothing wrote out, as a tuple value's is, is shown without names.
- A union is its members separated by `|`.
- A [type match](types.md#type-matches) is written as a match is, where a type is expected: `match`, the subject type, and arms of a pattern, `=>`, and a type. `let` and a name binds a type, as `match E { (K, let V) => V }`.

## Files

- A source file is a module and ends in `.ab`.
- What the reader does not yet keep, and so what cannot be written back, is listed under "Source" in [trees](trees.md).

## Positions

- Where a thing was written is not part of its [tree](trees.md): it never reaches a hash, and moving text re-checks nothing.
- The reader keeps, beside each definition, where every node of its body was written; a node the reader made up is where the text it was made for is.
- An error in a definition says which node it is at, and is reported at that node's line and column.
- A value that does not fit is wrong where it is given: at the `return`, not at the block that holds it.
- Checking says, beside the [core tree](trees.md) it makes, which node of the body each of its nodes was made from; like a position, that is no part of the tree.
- A frame of a [trace](errors.md) is therefore reported at a line and column too: its node of the core tree, the node of the body that was made from, and where that was written.
- A frame in a definition that was not read from text, as one of the standard library is not, is reported by name alone.
