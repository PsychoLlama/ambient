# Exchange

Definitions and values move between running programs by hash. A receiver trusts the code it is given, unless it embeds the compiler and rechecks it.

## Trust

- Code that reaches a program is trusted: that it is the definition its hash names, of the type it is said to have, compiled faithfully.
- A hash commits to a definition's type, so a definition known by hash when a program was built needs no check when it is loaded.
- A definition whose hash is learned as the program runs arrives as `unknown`, and a [cast](types.md) to the type the receiver expects is the check.
- A mismatch is therefore the cast's other branch where the definition arrives, never a fault where it is later used.
- What a function value handed to a program captured is trusted to be what its definition takes.
- Keeping what is delivered authentic is the program's concern.
- Running code from a sender that is not trusted takes the compiler: a host embeds it, rechecks what arrives, and vouches for the result itself.
- The language does no more for that case; such a host is assembled from the compiler as a library.

## Packs

- A pack is for a receiver that has the compiler; one that does not is given code compiled for its backend.
- A pack is the core trees of a definition and of the dependencies its receiver lacks.
- A pack names a definition it carries by position and one its receiver has by hash; it carries no hash of what it holds.
- A receiver recomputes every hash from the trees it received.
- A pack is independent of backend; each side compiles for its own.
- A received definition is checked against its signature before it is linked; the sender's compiler is not trusted.
- The check reads only core trees: a tree determines the type of each of its nodes, so nothing is inferred and no source is needed.
- A receiver accepts exactly the trees its own checker could have produced; a type written in any but its one form is refused.
- A received definition never names a native handler; only a program's root does.
- TODO: nothing bounds how deeply a received tree nests, and checking one recurses.
- An extern function in a pack resolves against the receiver's own native code by identity.
- A pack that needs an extern the receiver lacks is rejected before any of it runs.

## Receiving

- Decoding a function value needs its definition, which comes from one of two places.
- A receiver may supply the definitions it is willing to run; a hash outside that set does not decode, and nothing new enters the program.
- Or the definition is loaded, as compiled code or from a pack; loading adds code to a running program and is an ability.
- A loaded definition is held by the values that refer to it and is discarded with them.
- TODO: a loaded definition is kept until its program is dropped; cells, type arguments, and function values made by it all point into it, and nothing counts them yet.
- A definition the program already has is not loaded again: one hash is one definition.
- A loaded definition is never written to a [store](build.md).

## Values

- Any [unanchored](values.md) value can be sent.
- A sent function's row is its complete requirement on the receiver; handlers it captured travel with it as code.

## Running received code

- A receiver runs a received function under an only-mode [provide](handlers.md); the function reaches that bundle and nothing else.
- A receiver that runs the function inside a [boundary](values.md) and a [budget](budgets.md) also contains its faults and bounds its cost.
- Abilities match across programs by identity: two programs mean the same ability exactly when its tag and signature agree.
- Brands are not trusted across exchange, since received code may contain branding operations.
- Authority therefore rests on handlers and handles, never on brands.
