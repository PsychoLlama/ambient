# Exchange

Definitions and values move between running programs by hash. A receiver trusts nothing it has not recomputed or rechecked.

## Packs

- A pack is the core trees of a definition and of the dependencies its receiver lacks.
- A receiver recomputes every hash from the trees it received.
- A pack is independent of backend; each side compiles for its own.
- A received definition is checked against its signature before it is linked; the sender's compiler is not trusted.
- An extern function in a pack resolves against the receiver's own native code by identity.
- A pack that needs an extern the receiver lacks is rejected before any of it runs.

## Values

- Any [unanchored](values.md) value can be sent.
- A sent function's row is its complete requirement on the receiver; handlers it captured travel with it as code.

## Running received code

- A receiver runs a received function under an only-mode [provide](handlers.md); the function reaches that bundle and nothing else.
- A receiver that runs the function inside a [boundary](values.md) also contains its faults.
- Abilities match across programs by identity: two programs mean the same ability exactly when its tag and signature agree.
- Brands are not trusted across exchange, since received code may contain branding operations.
- Authority therefore rests on handlers and handles, never on brands.
