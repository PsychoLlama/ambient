# Testing

Testing is library code over one program the build writes: the tests of a run, handed to whatever runs them. Finding tests, running them, reporting them, and keeping their outcomes are each a function a package may replace.

## Tests

- A test is a function that takes nothing, a name, and the plain data it is handed.
- A name is a string and the group it is in, if any; a group has a name and a group of its own. A module is a group, and so is any grouping a library makes.
- A test reaches what its [row](abilities.md) names: what its outcome is made through provides, and the world of the [command-line target](targets.md#standard-targets). One that needs less provides it, which is how a test says what the world holds.
- A test whose row names nothing of the world is pure: it is a function of its [hash](definitions.md) and the native code it reaches.
- A test that does not compile is refused where it is found, and runs no other the worse for it.
- Importing a module runs nothing, so no test is made by being imported.

## Outcomes

- What a test came to is its outcome: each check that did not hold, and the [error](errors.md) that ended it, if one did. A [fault](values.md) ends it as an error does.
- An outcome is plain data. Its error's [trace](errors.md#traces) is of hashes, which a report names.
- A test passes if its outcome holds neither.

## Checks

- A test is run through a function that provides one ability, which is given a failure as a value: what did not hold, and whatever more it holds, as what was found and what was expected. Every assertion is a library function over it, and nothing written is one.
- A failure is open: an assertion library adds fields, and a reporter shows what it does not know as data. So assertions of several libraries mix in one test.
- A check that does not hold fails its test and the test goes on, so one run reports every check that does not hold; an error or a fault fails it and ends it.

## Finding

- Finding tests is a [tool](tools.md): it is handed a [selection](tools.md#commands) and gives the tests it covers, reading modules, definitions, and annotations through the build abilities.
- It is memoized as any tool is: adding, moving, or changing a test changes what it asked, so it runs again, and nothing is told to look.
- A package's manifest may name several, and its tests are those of all, so a package moves from one way of writing tests to another a module at a time.
- The standard one finds each function annotated as a test that takes nothing. Another may find tests by a module's path, by a definition's name, or by running a function that gathers specs, since each is something a build answers.
- Gathering specs is a run of its own and pure, and each spec it gives is a function with a [hash](definitions.md) of its own, so a spec is kept apart from those beside it.

## Running

- The tests of a run are compiled into one program, which is run as any program under the command-line target is: in the command's process, or [linked](linking.md#native-bundles) with the native bundles it reaches.
- The build writes the program's entry. It hands each package's runner that package's tests: each with its name, its group, the function that runs it to its outcome, and the key its outcome may be kept under; and each that was refused, with why.
- Changing a test compiles it and the entry again, and nothing else. Which tests a run chooses is told to the program as it starts, so choosing compiles nothing.
- A runner is library code. A package's manifest may name the function that runs its tests in place of the standard library's.
- Whether to reuse an outcome, keep a failure, try again, run several at once, or stop one that takes too long is the runner's to decide.
- A runner reads what a test's annotations say of how it is run, as a number of tries. What changes what a test sees, as [replay](handlers.md#recording) does, is a function the test is run through.
- A runner reports by an ability it performs for each event: a test begun, its outcome and whether it was reused, and the end of a run with how many passed, failed, and were reused. A reporter is a handler of it, and several are a bundle merged of theirs.
- The standard runner runs one test at a time, in order, and reuses a kept outcome but a failure of a test whose annotation says it may be tried again; a test that fails is run as many times as that says.
- The standard reporter writes each event as text, naming a trace as a program that ends in an error is reported.

## Keeping

- What testing keeps is kept apart from the [build](build.md): in a cache of bytes by key, under the build directory of the workspace, or of the package where there is none, by each package's ID. The build's store holds nothing of a test.
- The library's functions are the only way to the cache, so where it is is decided once. A runner keeps outcomes in it, and a test utility keeps what it chooses.
- Only a pure test's outcome is kept by its key: the hash of the test, the hash of what runs it, the build of the host, and the digest of each native bundle it reaches. A native function reaches no world its row does not name, so its bundle's digest stands for what it gives.
- A test that is not pure runs every time.
- Neither a test's name nor its group is in its key, and no outcome holds a name: a test moved or renamed unchanged is not run again, and is reported under the name it has now.
- A test that failed is kept as one that passed is, so a failure is reported again without running again, for as long as what caused it lasts.
- A wrapper a test is run through is part of the test's hash, so changing how a test is kept or replayed keeps it under another key: a change is a miss, never a stale read.

## Recordings

- A test of real effects is made repeatable by library code over [recording](handlers.md#recording): its first run records what the world answered, and later runs are answered from that.
- A recording is source, kept beside its test and read as any file of the package is.
- A test gives a file it would write as part of its outcome, and the command writes it. One that is there already is replaced only when the command is told to, so a replay that fails never records over what it was checked against.

## Commands

- A package that names no part of testing gets the standard library's, so nothing is written to test.
- A run may be of only the tests whose name or group holds a given text.

## Unbuilt

- TODO: tests run one at a time.
- TODO: a test is handed no data and gives no file.
- TODO: nothing limits a test's time, so a test that never ends never lets its run end.
- TODO: nothing takes away what is kept for a test no run keys any more.
