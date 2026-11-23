# Live Upgrade

A program upgrades while running by swapping the handlers its late-bound abilities reach. Live upgrade is a pattern over abilities, cells, and exchange; the language and runtime have no feature specific to it.

## Model

- A hash's behaviour never changes, so an upgrade never edits running code; it changes which handler a perform reaches.
- A late-bound point is an [ability](abilities.md) whose handler reads the current generation from a [cell](cells.md) and calls it.
- A generation is a [bundle](handlers.md) of handlers for a program's late-bound abilities.
- A deploy replaces the generation in the cell.
- There is no table of names at runtime and no rebinding of names; an ability's identity is what survives between generations.

## Consistency

- One perform reads one generation; everything beneath the handler it reaches is pinned by hash.
- Old and new generations coexist; a call in flight finishes on the generation it started in.
- Code upgrades only where it performs a late-bound ability; code that never does stays as it was.
- Which code can change is visible in rows.

## Compatibility

- A generation is [cast](types.md) to the bundle type the running program expects before it is swapped in.
- A generation whose handlers do not fit, or need abilities the program does not provide to them, is rejected and the previous one keeps running.
- Changing a late-bound ability's signature makes a different ability; the old one stays until nothing performs it.

## State

- State survives an upgrade because it is held by code above the late-bound points and handed down.
- Changing the shape of state means re-entering the code that owns it.
- An owner hands its state to its successor as `unknown`; the successor's cast is the migration.
- Handles and cells pass through that handover unchanged, since nothing is serialised.

## Structure

- A change to a program's tasks takes effect when the code that spawns them is re-entered.
- An owner that wants this waits on the generation cell, leaves its [group](tasks.md) by an abort, and re-enters through a late-bound ability.
- How much restarts on a deploy is therefore the author's choice, expressed in where the late-bound points are.

## Delivery

- A new generation arrives as a [pack](exchange.md) and is produced by one designated function, found by [symbol](symbols.md) in each build.
- The program checks and compiles it where it arrives, so a generation does only what its type says, with the handlers the program hands it.
- That symbol is the only name that must agree between builds.
- A development loop, a remote deploy, and an interactive session deliver generations the same way.
- Whoever can deliver a generation controls the program; guarding that is the program's concern.
