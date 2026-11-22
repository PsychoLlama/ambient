# Testing

Testing is library code over one thing the compiler gives: running a test as a program of its own, which is a [memoized run](memoization.md). Finding tests, running them, and reporting them are each a function a package may replace.

## Tests

- A test is a function of the build, the plain data it is handed, the [target](targets.md) it runs under, and a name.
- A name is a string and the group it is in, if any; a group has a name and a group of its own. A module is a group, and so is any grouping a library makes.
- A test runs under the standard library's test target unless an [annotation](annotations.md) on it names another, which is how a test of real effects runs under one that binds the world.
- A test whose [row](abilities.md) its target does not cover is refused where it is found, and runs no other the worse for it.
- Importing a module runs nothing, so no test is made by being imported.

## Outcomes

- What a test came to is its outcome: each check that did not hold, and the [error](errors.md) that ended it, if one did. A [fault](values.md) ends it as an error does.
- An outcome is plain data. Its error's [trace](errors.md#traces) is of hashes, which a report names.
- A test passes if its outcome holds neither.
- Neither a test's name nor its group is in the key of its run, and no outcome holds a name: a test moved or renamed unchanged is not run again, and is reported under the name it has now.
- A test that failed is kept as one that passed is, so a failure is reported again without running again, for as long as what caused it lasts.

## Checks

- The test target's world holds one ability, which is given a failure as a value: what did not hold, and whatever more it holds, as what was found and what was expected. Every assertion is a library function over it, and nothing written is one.
- A failure is open: an assertion library adds fields, and a reporter shows what it does not know as data. So assertions of several libraries mix in one test.
- A check that does not hold fails its test and the test goes on, so one run reports every check that does not hold; an error or a fault fails it and ends it.
- A test reaches no world its target does not bind. One whose function needs more provides it, which is how a test says what the world holds.

## Finding

- Finding tests is a [tool](tools.md): it is handed a [selection](tools.md#commands) and gives the tests it covers, reading modules, definitions, and annotations through the build abilities.
- It is memoized as any tool is: adding, moving, or changing a test changes what it asked, so it runs again, and nothing is told to look.
- A package's manifest may name several, and its tests are those of all, so a package moves from one way of writing tests to another a module at a time.
- The standard one finds each function annotated as a test that takes nothing. Another may find tests by a module's path, by a definition's name, or by running a function that gathers specs, since each is something a build answers.
- Gathering specs is a run of its own and pure, and each spec it gives is a function with a [hash](definitions.md) of its own, so a spec is kept apart from those beside it.

## Running

- Running tests is a program under a target of its own, which binds running one test, asking the [store](build.md) for the outcome a run of it left, and keeping one.
- Whether to reuse an outcome, keep a failure, try again, run several at once, or stop one that takes too long is the runner's to decide.
- What the store gives is an outcome whose [trace](memoization.md) still holds, so a runner chooses only whether to reuse: none can make one stale.
- A runner reads what a test's annotations say of how it is run, as a time limit or a number of tries. What changes what a test sees, as [replay](handlers.md#recording) does, is a function the test is run through.
- A runner reports by an ability it performs for each event: a test begun, its outcome, and the end of a run. A reporter is a handler of it, and several are a bundle merged of theirs.

## Recordings

- A test of real effects is made repeatable by library code over [recording](handlers.md#recording): its first run records what the world answered, and later runs are answered from that.
- A recording is source, kept beside its test and read as any file of the package is; the store holds only outcomes, and whoever points a build at a store trusts it.
- A run answered from a recording asks only for a file, so it is reused as a test that reaches no world is, though the test's row names the world.
- A test gives a file it would write as part of its outcome, and the command writes it. One that is there already is replaced only when the command is told to, so a replay that fails never records over what it was checked against.

## Commands

- A package that names no part of testing gets the standard library's, so nothing is written to test.
- A run may be of only the tests whose name or group holds a given text.

## Unbuilt

- TODO: little of this is built. A test is a function that takes nothing and whose name begins with `test_`, found by the command line, which runs, keeps, and reports tests itself; no package replaces any of it.
- TODO: a test runs only under the standard test target, and gives no file.
- TODO: a test reaches no [native bundle](externs.md#native-bundles), so a test of a package that brings native code cannot run what it brings.
- TODO: a limit on a test's time cancels it, and a test that never suspends is never cancelled.
