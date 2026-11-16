# Errors

An error is a structured value of one standard shape, and it is the only thing that can be thrown. Errors are for failures a program expects; a [fault](values.md) is not an error until a boundary makes it one.

## Shape

- `Error` is a record type with a kind, a message, a cause, and a trace.
- The kind is a [tag](definitions.md) value; it identifies what went wrong and is what code matches on.
- The message is text for a person; nothing parses it.
- The cause is another error or `null`.
- `Error` is an interface, not a closed set: any record with these fields is an error, and a specific error adds its own fields.
- Data about a failure lives in fields, never in the message.

## Throwing

- `throw` is an [ability](abilities.md) that takes an `Error` and returns `never`; no other value can be thrown.
- Failing is a function that throws an error of a kind and a message, made where it is called, so its [trace](#traces) begins there.
- Catching is a function that takes a function and gives what it gives or the error it threw; it is a [provide](handlers.md) and nothing more.
- An ability other than `throw` that returns `never` is control flow, not an error, and carries whatever its signature says.
- A [boundary](values.md) reports the fault it contained as an `Error`.
- A program's root is a boundary: a fault that ends a program reaches its host as the same `Error`.
- The kind of such an error is a tag the standard library declares, one for each fault, beside `Error`.

## Traces

- A trace is a list of frames, each a definition hash and a node position within that definition.
- A node's position is its place in a walk of the definition's core tree that visits each node before the nodes inside it.
- A trace is gathered as a [fault](values.md) or an [abort](handlers.md) unwinds: a frame for the node it began at and for each call it leaves, nearest first. A task's frames are followed by those of what started it.
- It ends where the unwinding does: a fault's at the boundary that contains it, an abort's at its provide.
- A function that left for a [tail call](backends.md) is in no trace.
- A trace contains no names; a tool with the [index](symbols.md) turns frames into symbols and source positions.
- A trace is data: it serialises with its error and can be resolved on another machine.
- Reading the trace of the abort a handler is answering is a language operation, not an ability. Anywhere else it gives no frames.
- An error is made with no trace. Catching gives it the trace of the throw that brought it, after any it had, so one thrown again says both ways it came.
- An error that is only ever a value has no trace, and costs none.
- Nothing keeps a record of calls as they are made, so code that never unwinds pays nothing for traces. A target may bound how many frames one keeps.
