# Exchange

Definitions and values move between running programs by hash. A receiver trusts nothing it has not recomputed or rechecked.

## Packs

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
- Or the definition arrives in a pack and is loaded; loading adds code to a running program and is an ability.
- A loaded definition is held by the values that refer to it and is discarded with them.
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
