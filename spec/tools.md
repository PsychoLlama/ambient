# Tools

A tool is a function a build runs to make something other than a program's definitions: the files of a build, a list of diagnostics, a report. What concerns one definition is a [comptime](comptime.md) call, and what concerns many is a tool.

## Tools

- A tool is an ordinary function whose [row](abilities.md) is of the [build abilities](#build-abilities). It runs under the build [target](targets.md), as a comptime call does.
- A tool is a [memoized run](memoization.md): it runs again only when something it asked is answered differently.
- A tool gives a value of a standard shape and writes nothing. Whatever ran it decides what becomes of the value.
- A tool is handed what it is to work on as arguments, a target included, so all of it is in the run's key.
- A pipeline is a function that calls others. Nothing is matched by a file's name or its content, so no step can be fed what it gave.
- A part of a tool is cached on its own only where the tool asks for it as a run of its own.
- A library function a tool calls performs through the tool's bindings, so it reads the files of the tool's package, under that package's grants.
- A tool is library code: a package exports one as it exports any function, and the compiler holds none.

## Build abilities

- The standard library declares the build abilities. Each is a narrow question, so a run's trace holds only what its result depends on.
- They are [verifiable](memoization.md) unless this section says otherwise.
- The files of a package: reading one as text or as bytes, and listing a directory. Which files those are is the [package's](packages.md#layout) to say.
- The build target binds none of the [`system`](targets.md#the-system-package) abilities, whose files are those of the machine a program runs on.
- A definition: its tree, its interface, its [annotations](annotations.md), and its [metadata](trees.md). A module's definitions, and a package's modules.
- What a definition reaches: its definitions, its externs, and the annotations of a key, [in order](annotations.md#collecting).
- Compiling an entry under a target for a backend, which gives the module and the externs it imports.
- An extern's [host code](externs.md#distribution) for a platform, from the package that declares it.
- Running a function under a target, as a memoized run of its own, which gives its result or its fault.
- The bytes at a URL, given their digest. The digest is what is verified, so what was fetched once is never fetched again.
- What differs from one build to the next, as the time does, is asked through volatile abilities. A run that asks is run on every build, and one that gives what it gave before invalidates nothing.
- A comptime call may perform these too. One that asks about what reaches its own definition is the [cycle](comptime.md#calls) every such call is refused for.

## Builds

- A [target](targets.md) names the tool that builds a program under it. That tool is handed the entry and the target, and gives files: each a path and its bytes.
- A target that names none has the tool of the target it extends. One with none at all is built as its module alone.
- The command line writes the files into a directory it owns, and removes from it what the build no longer gives.
- A build step is a tool that adds files to a build: it is handed the entry and the target, and gives files.
- A package declares its build steps in its manifest, each for a target. One runs in a build whose target is that target or extends it, and whose entry reaches the package.
- A package that owns an annotation's key therefore owns the one step that collects it, from every package of the program. No dependent declares or runs a step for it.
- No step sees what another gave. The target's tool runs after them and is handed what they gave, so it is what ties the files together.
- Two files given for one path are an error.
- A step is run for the target it declares and for no other, so a package that supports several targets costs a build only the one it is for.
- That a package declares a build step needs its workspace's [approval](releases.md#approvals), as its build grants do.

## Late names

- No [artifact](build.md) feeds a hash, so nothing a build makes is content of what it was made from.
- A file a comptime call names is named by the digest of its content, which is known before anything is built.
- A value known only when a build is done reaches the program as an [extern](externs.md) whose host code the target's tool writes: the program's own hash, a table of its files.

## Commands

- The command line's commands and their flags are fixed, whatever tools are behind them.
- A command hands a tool a selection: the packages and modules to cover, and text a name must hold. What is selected is the command's to decide, so every tool honours the same flags.
- The standard library defines the shapes tools give: files, a selection, and a diagnostic, which is what [checking](packages.md#running) reports with.
- A lint is a function of a standard type that gives diagnostics, over one definition or over a package. A package's manifest lists the lints it wants beside the standard ones.
- A package that names nothing gets the standard tools, so none is written to check, test, or build.

## Unbuilt

- TODO: a tool reads its package's files, compiles an entry, reads host code, and asks for the annotations a definition reaches and for the type of a function or an extern, and nothing else: no tree of a definition, no reach but of annotations and of the externs compiled, nested run, or fetch. Only a target's tool runs: there are no build steps, lints, or selections.
- TODO: [testing](packages.md#testing) is not a tool: its harness is the command line's, so no package replaces it.
