# Constants

A constant is a definition whose content is a value. It is known when it is checked, held by a program as data, and never computed as a program runs.

## Definition

- A constant is a name, a type, and a value, declared in a [module](modules.md) and nowhere else. A binding is the constant of a block.
- Its [interface](definitions.md) is the type it states, and its value is held to that type. The type is never read from the value, so a change of value rechecks nothing that names the constant.
- A constant has no [identity](definitions.md): its hash covers its type and its value, so two constants that state one type and hold one value are one definition.
- A constant takes no type parameters.
- Naming a constant gives its value. That is a read: nothing is called and nothing is made.
- A constant is a [symbol](symbols.md) of the value namespace, and is imported, exported, and aliased as a function is.

## Values

- A value is written in value forms: a literal, `null`, a [tag](definitions.md), a record, a tuple, or a list of values, a [branded](types.md) value, the name of another constant, the name of a function, and a lambda.
- No other expression is one: a value holds no call, perform, [operation](operations.md), match, or binding.
- A [comptime](comptime.md) call may stand where a value may, and what it gives is spliced before the value is read. Whatever a constant computes is therefore computed by a build, where the call says so.
- Nothing is in scope around a constant but definitions, so a lambda in one captures no local and no [binding](abilities.md): what it performs is in its row.
- A function named at type arguments that mention no type parameter is a value too.
- A value is therefore never [anchored](values.md). No constant holds a [cell](cells.md) or a handle, so no state and no authority is reached by a name.
- A constant names itself only inside a function its value holds, and the two are a [group](definitions.md). One whose value holds itself any other way is an error.

## Cost

- A constant's value is made once, when the code that holds it is [loaded](linking.md), and is data from then on. A program carries only the constants its entry reaches.
- A value read from a constant is [shared](values.md) as any value is, and is told from no other: it [encodes](values.md) as what it holds, with nothing of the constant.
- One hash is one definition, so equal constants are one value in a program, and loaded code brings no constant its target has.
- A constant has no code: no [backend](backends.md) compiles one, and what reads one reaches it through a [slot](core.md).
- Editing a value therefore rechecks that constant, gives what names it new hashes, and compiles nothing.

## Stages

- A constant is read, resolved, checked, and hashed as a function is, under keys of the same kind; the [store](build.md) and the [index](symbols.md) hold one as they hold any definition.
- A constant whose value came from a comptime call that observed anything is kept with that call, [as a function is](memoization.md#dependents).
- A [memoized run](memoization.md) is keyed by its program's hash, which covers every constant the program reaches.
- A constant carries [annotations](annotations.md) as any definition does, and its value may hold annotated expressions. Both are content, and a program holds neither.
- A [pack](exchange.md) carries a constant as it carries any definition, and a receiver holds its tree to the value forms.

## Platforms

- No constant is supplied by native code. Its value would be unknown where it is checked, and an [artifact](build.md) would differ by the native code it is loaded with.
- What only a platform knows is an [extern](externs.md) function of no parameters.
- A constant is the same under every [target](targets.md), since a comptime call is never told what a program is built for.
