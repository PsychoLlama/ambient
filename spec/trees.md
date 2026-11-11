# Trees

A program is a set of trees constructed through builders. Text syntax is deferred; a parser will be one more client of the builders.

## Surface and core

- A surface tree is what an author writes: names are unresolved, comptime calls unexpanded, captures implicit.
- A core tree is what checking produces: references are hashes, captures and bindings are explicit, comptime results are spliced, lambdas are separate definitions.
- The core tree is the canonical form: it is what is hashed, stored, exchanged, and compiled.
- Every language construct is a tree node; nothing exists only as syntax.

## Builders

- Builders are the first interface to the language and remain the primary one.
- The test suite, embedders, comptime functions, and migrations all construct programs through builders.
- A builder rejects an ill-formed tree at construction; type errors are left to the checker.
- The same builders are available from the host language and from Ambient.

## Metadata

- Names, doc comments, and source positions are attached to nodes and are not tree content.
- A node is addressed by its position within its definition, which reformatting cannot change.
- Every definition, parameter, and binder may carry a doc comment.

## Syntax

- A grammar is specified only after the trees are stable.
- The grammar maps surface trees to text and back without loss, so a tool can rewrite source by rewriting trees.
