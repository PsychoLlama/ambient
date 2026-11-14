# Syntax

Source text is braces and statements, read by anyone who reads TypeScript, Rust, or Swift. Every form here is a way of writing a [tree](trees.md) node or a fixed arrangement of them; nothing exists only as syntax.

## Statements

- A statement ends in `;`, and nothing is inserted where one is missing.
- A block is statements between braces and never has a value.
- `return` gives a body its value and is always written; a body that ends without one gives `null`.
- A body is a block, or `=>` and one expression, wherever a body is written.
- A brace where a value is expected is therefore always a record.
- `let` binds a name, or the names of a tuple or record pattern, for the rest of its block.
- An expression may stand as a statement; its value is dropped.

## Returning early

- The language has no node for leaving a function early, so a `return` is always the last thing some path through its body does.
- An `if` or a match written as a statement may return in some branches and not others; the statements after it are then what the other branches go on to do.
- The bodies of a boundary, a budget, a provide, a recording, a group, and a task are bodies like a function's: `return` gives that construct its value.
- Nothing returns across one of those, since nothing but an [abort](handlers.md) leaves one early.

## Definitions

- A definition is written with the keyword of its [kind](definitions.md): `fn`, `type`, `ability`, `key`, `tag`, `extern`, `target`.
- A word that only begins a definition is not reserved: `key`, `tag`, `ability`, `target`, `extern`, `import`, and `module` may name anything, since nothing else begins where a definition does.
- `pub` before one makes it public, and a `///` comment before one is its doc comment.
- No definition is given a string to be known by: [identity](definitions.md) is read from where it is declared.
- An `import` names a module, or a module and the definitions taken from it in braces; `pub import` re-exports.
- Type parameters follow a name in angle brackets, each with an optional bound after a colon.
- A function's row follows its return type after `with`.
- There is no form for a constant: it is a function of no parameters, called.

## Expressions

- A call is a callee and arguments in parentheses; a method call puts a receiver and a dot before it.
- A [perform](abilities.md) is written as a call with `!` after the ability, so every place a function reaches outside itself is visible where it does.
- A lambda is `fn`, parameters, and a body.
- A record is members between braces: a field, a field named for a local of the same name, a function or method written with its parameters, or `...` and a record to [merge](methods.md) over.
- A member whose first parameter is `self` is a [method](methods.md); with any other first parameter it is a function.
- A record names a [key](methods.md) or an [ability](handlers.md) in square brackets, where it would name a field, and indexing by one in square brackets reads it.
- A [cast](types.md) is a value, `as`, a type, `else`, and what it is otherwise.
- A [comptime](comptime.md) call is `comptime` before the call.
- Type arguments are written out after `::` in angle brackets.
- An [operation](operations.md) with no operator is `@`, its name, and its operands; the standard library wraps each in a function, and other code calls that.
- Operators are those of [operations](operations.md) and, built from them, the other comparisons, `&&`, `||`, and `!`; the last three evaluate only the operands they need.
- `if` is a statement, and a match is the conditional that has a value.
- A condition is matched like any other subject, so one whose type is a single literal has a branch that can never be taken, which is an error.

## Patterns

- A match used as a value has an expression for each arm, and one used as a statement has a block for each.
- A bare name in a pattern is a type or a [tag](definitions.md), never a binder, so a misspelt name is an error and not an arm that matches everything.
- `let` and a name binds what an arm matched; a colon and a pattern after it narrows what is bound.
- A pattern after `let` in a statement binds its bare names, since nothing there is tested.

## Scopes

- A [boundary](values.md) is `try`, a [budget](budgets.md) is `budget` and its limits, a [provide](handlers.md) is `provide` and a bundle, each followed by a body.
- `provide only` is a provide in only-mode, and `observe` and an observer is a [recording](handlers.md).
- `using` and a binding registers a [cleanup](cleanup.md).
- A [group](tasks.md) is `group` and a body, in which `spawn` and a body starts a task.
- An all and a first take their branches as functions of no parameters, so nothing written in one reads as running in order.

## Types

- A record type is written as a record is, with a type where a value would be.
- A member written with parameters whose first is `self` is a method type; a type after `self` is what it needs of its receiver.
- A function type is `fn`, parameter types, a return type, and a row after `with`.
- A union is its members separated by `|`.

## Files

- A source file is a module and ends in `.ab`.
- TODO: the reader keeps no [metadata](trees.md) but doc comments, so a file cannot yet be written back from its trees, and an error names a definition and not a place in it.
