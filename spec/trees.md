# Trees

A program is a set of trees constructed through builders, and its source text is a lossless rendering of them. A [parser](syntax.md) is one more client of the builders.

## Surface and core

- A surface tree is what an author writes: names are unresolved, comptime calls unexpanded, captures implicit.
- A [core tree](core.md) is what checking produces: references are slots, captures and bindings are explicit, comptime results are spliced, lambdas are separate definitions.
- The core tree is the canonical form: it is what is hashed, stored, exchanged, and compiled.
- Every language construct is a tree node; nothing exists only as syntax.
- A surface tree has two levels: the tree as written, where each shorthand is a node, and the tree it stands for, which is a pure function of the written one.
- Tools read and edit the written tree; resolving and checking see only what it stands for.

## Builders

- Builders are the first interface to the language and remain the primary one.
- The test suite, embedders, comptime functions, and migrations all construct programs through builders.
- A builder rejects an ill-formed tree at construction; type errors are left to the checker.
- The same builders are available from the host language and from Ambient.

## Metadata

- Names, doc comments, comments, literal spellings, and spans are metadata: attached to nodes, never tree content.
- A span is where a node is in its file, from its first byte to its last; a node that was built has none.
- Every comment belongs to exactly one node, so moving a node moves its comments.
- Whitespace belongs to no node: it is what a file holds between spans.
- A span means something only against the file it was read from; the [store](build.md) and the [index](symbols.md) hold spans, never the text they cover.
- A node is addressed by its position within its definition, which reformatting cannot change.

## Doc comments

- Anything that introduces a name may carry a doc comment: definitions, parameters, type parameters, local binders, record fields, and aliases.
- A doc comment on a record field belongs to the site that wrote it, not to the type, since equal types share a hash.
- Doc comments are written in one specified Markdown dialect.
- Doc comments are available to tools through the [index](symbols.md) and to comptime functions as [metadata](comptime.md).

## Source

- Source files are authoritative; the [store](build.md) is derived from them.
- A node nothing changed is written back as its span of the file, so reading a file and writing it back reproduces it byte for byte.
- A node with no span, or one that was changed, is written in the canonical format.
- The grammar is provisional while the trees are not stable; whatever it becomes satisfies this section and the next.
- TODO: the reader arranges each shorthand into what it stands for as it reads, so there is no written tree, and an `if` or a `for` could not be written back as it was.
- TODO: a node's span is only where it starts; declarations, parameters, patterns, and imports have none, and comments other than doc comments are dropped.
- TODO: nothing writes a tree as text, so there is no edit and no formatter.

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

- Formatting rewrites the whitespace between tokens and nothing else; it cannot change a tree, a comment, a literal spelling, or a hash.
- The canonical format is a function of the tree, its comments, and whether a blank line precedes each sequence item; no other whitespace an author wrote survives it.
- There is one canonical format and it has no options.
- Formatting needs only a parse, so code that does not resolve or check still formats.
- Formatting is idempotent.

## Edits

- An edit reads a definition by symbol as a surface tree with metadata, transforms it as data, and writes it back to its source file.
- An edit changes only the nodes it touched; everything outside them survives byte for byte.
- A definition with no source file, such as one a host builds, is read as a tree but never edited.
- Comments and doc comments are data to an edit like any node, and can be added, changed, or removed.
- An edit names the version of the definition it read and is rejected if the source has since changed.
- An edit is addressed by symbol, never by file offset, so it can be issued by a tool that never sees the file.
- Text editing and tree editing are two views of one program; neither loses what the other wrote.
