# Trees

A program is a set of trees constructed through builders, and its source text is a lossless rendering of them. A [parser](syntax.md) is one more client of the builders.

## Surface and core

- A surface tree is what an author writes: names are unresolved, comptime calls unexpanded, captures implicit.
- A [core tree](core.md) is what checking produces: references are slots, captures and bindings are explicit, comptime results are spliced, lambdas are separate definitions.
- The core tree is the canonical form: it is what is hashed, stored, exchanged, and compiled.
- Every language construct is a tree node; nothing exists only as syntax.

## Builders

- Builders are the first interface to the language and remain the primary one.
- The test suite, embedders, comptime functions, and migrations all construct programs through builders.
- A builder rejects an ill-formed tree at construction; type errors are left to the checker.
- The same builders are available from the host language and from Ambient.

## Metadata

- Names, doc comments, comments, whitespace, literal spellings, and source positions are metadata: attached to nodes, never tree content.
- Every comment and every run of whitespace belongs to exactly one node.
- A node is addressed by its position within its definition, which reformatting cannot change.

## Doc comments

- Anything that introduces a name may carry a doc comment: definitions, parameters, type parameters, local binders, record fields, and aliases.
- A doc comment on a record field belongs to the site that wrote it, not to the type, since equal types share a hash.
- Doc comments are written in one specified Markdown dialect.
- Doc comments are available to tools through the [index](symbols.md) and to comptime functions as [metadata](comptime.md).

## Source

- Source files are authoritative; the [store](build.md) is derived from them.
- A source file and its surface trees with metadata determine each other exactly.
- Reading a file and writing it back unchanged reproduces it byte for byte.
- A node built without metadata is written in the canonical format.
- A grammar is specified only after the trees are stable, and must satisfy this section and the next.

## Text

- A file parses on its own; parsing never depends on resolution, types, or another file.
- Each surface node has exactly one spelling: two texts for the same tree differ only in whitespace, comments, and literal spellings.
- There is no optional punctuation, alternative keyword, or interchangeable delimiter.
- Where a shorthand exists it is its own node, so no tool chooses between a shorthand and its expansion.
- Whitespace separates tokens and means nothing else; no newline or indentation is significant.
- A sequence is any ordered run of items: definitions, statements, fields, parameters, arguments, arms, elements.
- Every sequence uses one convention for delimiters and separators.
- A comment leads or trails an item of a sequence, or ends a sequence; it can be written nowhere else.

## Formatting

- Formatting rewrites whitespace metadata and nothing else; it cannot change a tree, a comment, a literal spelling, or a hash.
- The canonical format is a function of the tree, its comments, and whether a blank line precedes each sequence item; no other whitespace an author wrote survives it.
- There is one canonical format and it has no options.
- Formatting needs only a parse, so code that does not resolve or check still formats.
- Formatting is idempotent.

## Edits

- An edit reads a definition by symbol as a surface tree with metadata, transforms it as data, and writes it back to its source file.
- An edit changes only the nodes it touched; every other comment and whitespace run survives byte for byte.
- Comments and doc comments are data to an edit like any node, and can be added, changed, or removed.
- An edit names the version of the definition it read and is rejected if the source has since changed.
- An edit is addressed by symbol, never by file offset, so it can be issued by a tool that never sees the file.
- Text editing and tree editing are two views of one program; neither loses what the other wrote.
