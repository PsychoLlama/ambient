# Annotations

An annotation is data a build reads and a program never does. It is how a definition says something to a [tool](tools.md) without any call collecting what another said.

## Shape

- An annotation is a key and a value: the key is a [tag](definitions.md), and the value is plain data, as a literal is.
- An annotation is on an expression or on a definition. Any definition may carry one, an [extern](externs.md) and an [ability](abilities.md) included, which have no body to hold one.
- One on an expression is a node of the [core tree](core.md) over that expression, and a program that runs sees the expression alone.
- An annotation is content: the [hash](definitions.md) of the definition it is in covers it. It is therefore kept, cached, and [exchanged](exchange.md) with its definition, and nothing else holds it.
- No backend reads an annotation. A compiled program holds none, and changing one compiles nothing again.
- An annotation is written by naming its key, so only a package that can name the tag writes one of it or asks for those written.
- A [comptime](comptime.md) call gives annotations in the tree it gives, which is how what one call found reaches a tool. One written by hand is no different.
- An annotation holds bytes by their digest; the [store](build.md) keeps the bytes, so a large file is never part of a tree.
- An annotation may hold a reference to a function where a value would be costly to make. The tool that collects it runs the function, so one nothing collects costs nothing.

## Collecting

- A tool asks for the annotations of a key that a definition reaches: those on each definition reached, and those in their trees.
- They are given in the order a walk from that definition first meets them: a tree in the order it is written, and a reference followed where it is first met.
- That order is a function of content alone, so an empty store gives what a full one does.
- A definition met twice gives its annotations once.
- Reach is by definition, so an annotation in a branch that is never taken is still given.
- An annotation in a generic function is one annotation, whatever types the function is used at.
- A tool may instead ask for the annotations of a key on the definitions of one module: what each is itself annotated with, in the order they are written, and nothing of what it reaches.
- That asks less, so what asks it is run again for less: it is how a module's definitions are chosen among, where reach is how a program's are gathered.
- A target's annotations are read only by the tools that ask for their key, so a program built for one target pays nothing for what its libraries say to another.

## Unbuilt

- TODO: nothing written annotates an expression: only a tree a comptime call gives does.
