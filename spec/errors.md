# Errors

An error is a structured value of one standard shape, and it is the only thing that can be thrown. Errors are for failures a program expects; a [fault](values.md) is not an error until a boundary makes it one.

## Shape

- `Error` is a record type with a kind, a message, a cause, and a trace.
- The kind is a [tag value](definitions.md); it identifies what went wrong and is what code matches on.
- The message is text for a person; nothing parses it.
- The cause is another error or `null`.
- `Error` is an interface, not a closed set: any record with these fields is an error, and a specific error adds its own fields.
- Data about a failure lives in fields, never in the message.

## Throwing

- `throw` is an [ability](abilities.md) that takes an `Error` and returns `never`; no other value can be thrown.
- An ability other than `throw` that returns `never` is control flow, not an error, and carries whatever its signature says.
- A [boundary](values.md) reports the fault it contained as an `Error`.
- A program's root is a boundary: a fault that ends a program reaches its host as the same `Error`.
- The kind of such an error is a tag of the language's own package, one for each fault.
- The standard library defines `Error` and a type for each of those tags; a program names a fault's kind through them, since a tag it writes is one of its own package.

## Traces

- A trace is a list of frames, each a definition hash and a node position within that definition.
- A node's position is its place in a walk of the definition's core tree that visits each node before the nodes inside it.
- The trace of a fault has a frame for the node that faulted and for each call the fault left, nearest the fault first; a task's frames are followed by those of what started it.
- A trace contains no names; a tool with the [index](symbols.md) turns frames into symbols and source positions.
- A trace is data: it serialises with its error and can be resolved on another machine.
- Capturing a trace is a language operation, not an ability; a target may bound how much it captures.
- The trace is captured where the error is made, not where it is thrown.
- A captured trace has a frame for the capture and for each call it is inside, ordered as a fault's is.
- A capture costs in proportion to how deeply the task is nested in calls; code that captures nothing pays nothing for it.
- Nothing else runs between a task asking for its trace and getting it.
