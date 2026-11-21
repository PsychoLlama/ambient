# Writing a program in Ambient

This is a tour for someone who wants to write and run something. The [spec](spec/README.md) says what the language is and why; [native bundles](native.md) are how a package reaches code that is not Ambient; the packages under [`examples/`](../examples) are complete programs.

Every block marked `ab` on this page is a whole module, and a test checks each one.

## A package

A package is a directory with a manifest and its modules under `src/`:

```
hello/
  ambient.toml      its name, its ID, its version, and what it runs
  src/
    main.ab         the module `main`: what the package runs
    text/trim.ab    the module `text.trim`
    lib.ab          what other packages see, if any do
```

```ab
use system.console;

pub fn main(): null with console.print {
    console.print!("hello, world");
}
```

```
ambient init hello           # makes a package that runs and has a test; `--lib` makes a library
cd hello                     # a command is for the package it is run in
ambient run                  # runs the binary `main`: `main` of the module `main`
ambient run -- one two       # what follows `--` is what env.args gives
ambient run --bin fmt        # runs another binary of the package; `--bin` alone says which there are
ambient check                # checks every definition, runs nothing
ambient test                 # runs every function named test_*
ambient build                # `build/hello`, one file that runs `main`: no package, no compiler
```

A command finds its package where it is run or above, so it works from `src/` as well.

What a package runs is a binary: a function of `main.ab`, which the manifest declares under the function's name. `main` needs no declaring; it is declared to say more of it.

```toml
[package]
name = "hello"
id = "f990a7d1ea86238e6f887caf27d32800"
version = "0.1.0"
description = "Says hello"   # for whoever reads it

[bin.main]
name = "hello"               # what `ambient build` calls the file in `build/`; `main`, if not said

[bin.fmt]                    # `fmt` of `main.ab`, which may call into any module
target = "pkg.tools.world"   # the target it runs under; the command line's, if not said
```

Nothing else of a package is run from the command line, so its manifest says all it offers. A `main` that gives an integer exits with it. What is built is kept by its content in one store for every package on the machine, so a second run compiles only what changed.

## Functions and values

A body is a block of statements, or `=>` and one expression. A block has no value of its own: `return` gives one, and it is always written.

```ab
const BIG: int = 10;

fn double(n: int): int => n * 2;

fn describe(n: int): string {
    let twice = double(n);
    if twice > BIG {
        return "big";
    }
    return "small: ${twice}";
}

pub fn main(): string => describe(3);
```

- The values are `null`, booleans, integers, floats, strings, tags, tuples `(1, "a")`, lists `[1, 2]`, records `{ name: "tea", price: 450 }`, and functions.
- A `const` names a value for the whole module: `const BIG: int = 10;`. Its name is in capitals, its type is stated, and its value is written out, not computed; what must be computed is a `comptime` call.
- Nothing is changed in place. `list.push(x)` gives another list; a record is copied with `{ ...old, price: 500 }`.
- `+` joins two strings, two lists, or two bytes values, and `"${count} left"` writes a value into a string by its `to_text`. Arithmetic on integers faults on overflow and division by zero instead of wrapping.
- There are no loops. A list or an iterator has `map`, `filter`, `fold`, `each`, and the rest; anything else is a function that calls itself, and a call in tail position takes no stack.
- `if` is a statement. The expressions that choose are `match` and `condition ? this : that`.

## Types and match

Types are structural: a record fits wherever its fields do, whatever it is called. A union is written with `|`, and `match` is the only thing that narrows one.

```ab
tag circle;
tag box;

type Shape = { kind: circle, radius: float } | { kind: box, width: float, height: float };

fn area(shape: Shape): float =>
    match shape {
        { kind: circle, let radius } => radius * radius * 3.14159,
        { kind: box, let width, let height } => width * height,
    };

fn first_word(text: string): string {
    match text.split(" ").first() {
        null => { return "nothing"; }
        let word => { return word; }
    }
}

pub fn main(): string => first_word("tea for two") + area({ kind: circle, radius: 1.0 }).to_text();
```

- In a pattern a bare name is a type or a tag to test for, and `let name` binds. After `let` in a statement, bare names bind: `let (a, b) = pair;`.
- A match used as a value has expressions for arms; used as a statement it has blocks.
- Absence is `null` in a union (`string | null`), and there is no exception to catch: an operation that can fail gives a union with `Error`.
- `value ?? 0` is the value, or `0` where it is null. `cast::<int>(value, 0)` is the value if it is an `int`, and `0` if not: a cast narrows by a type and says what to give otherwise. What it gives otherwise is evaluated either way, so one that should only happen on a failure goes after `??`: `cast::<int>(value, null) ?? fail(not_a_number, "…")`.
- `??` binds looser than every operator, so parenthesise it inside a larger expression.
- Literals have their own types: `1` is of type `1`, which is why a condition that is always `true` is an error. This rarely shows; where it does, say the wider type.

A tag is a named value with a type of its own, for the cases of a union:

```ab
tag paid;
tag declined;

type Outcome = paid | declined;

fn settle(amount: int): Outcome {
    if amount < 1000 {
        return paid;
    }
    return declined;
}

pub fn main(): string =>
    match settle(450) {
        paid => "thanks",
        declined => "sorry",
    };
```

## Records with methods

A member of a record whose first parameter is `self` is a method, called with a dot. Methods are how a type carries its operations; there are no classes.

```ab
type Counter = {
    count: int,
    bump(self): Counter,
    show(self): string,
};

fn counter(count: int): Counter => {
    count,
    bump(self): Counter => counter(self.count + 1),
    show(self): string => "at ${self.count}",
};

pub fn main(): string => counter(0).bump().bump().show();
```

The built-in shapes have methods too, written in the standard library: `"a b".split(" ")`, `[3, 1].sort_by(fn(a, b) => a < b)`, `7.to_text()`.

## Abilities

A function cannot touch the world. What it can do beyond computing is named in its header after `with`, and performed with `!`:

```ab
use system.console;
use system.fs;
use ambient.Error;

/// Prints a file, or why it could not.
fn show(path: string): null with console.print, fs.read {
    match fs.read!(path) {
        let error: Error => { console.print!("cannot read: ${error.message}"); }
        let text: string => { console.print!(text); }
    }
}

pub fn main(): null with console.print, fs.read {
    show("notes.txt");
}
```

- An ability is a declaration: `pub ability read(path: string): string | Error;`. It says nothing about how.
- The program's target supplies the real ones. `ambient run` gives `console`, `env`, `fs`, `clock`, and `process` from the standard library.
- A module in a row is every ability of it: `with console, fs`. Name the abilities themselves where it matters which.
- A function that lists no abilities is pure: it can be cached, run at compile time, or sent to another machine, and its header says so.
- A lambda takes the abilities in scope where it is written, so `list.each(fn(x) => console.print!(x))` needs nothing said.
- `for x in list { console.print!(x); }` is that same call written as a statement. Nothing leaves a `for` early; to stop at an item use `find`, `some`, or `fold`.

Any code can supply an ability to what it calls, which is how a test replaces the world:

```ab
use system.console;

fn greet(name: string): null with console.print {
    console.print!("hello, ${name}");
}

/// What `greet` prints, without printing it.
pub fn main(): string {
    let said = cell::<string>("");
    let quiet = {
        [console.print](text) { said.update(fn(so_far) => so_far + text); },
    };
    provide quiet {
        greet("you");
    }
    return said.read();
}
```

A cell is the one thing that changes: `cell::<T>(value)` makes one, with `read`, `update`, and `replace`.

## Declaring your own

```ab
/// Asks whoever is in charge whether to go on.
ability confirm(question: string): bool;

fn remove(path: string): string with confirm {
    if confirm!("delete ${path}?") {
        return "deleted";
    }
    return "kept";
}

pub fn main(): string {
    let always = { [confirm](question) => true };
    return provide always => remove("old.txt");
}
```

## Failing

- A fault (dividing by zero, overflow) unwinds. `contain(fn() { ... })` stops it and gives an `Error` value instead, with `kind`, `message`, and `trace`.
- `using name = value;` binds a value that carries its own cleanup, which runs when the block ends however it ends.

```ab
use ambient.cleanup.{dispose};
use system.console;
use ambient.Error;

fn risky(by: int): int => 100 / by;

fn noisy(name: string): { [dispose](): null } with console.print =>
    { [dispose]() { console.print!("closed ${name}"); } };

pub fn main(): string with console.print {
    let outcome = contain(fn() {
        using _ = noisy("the file");
        return risky(0);
    });
    match outcome {
        let error: Error => { return "failed: ${error.message}"; }
        let answer: int => { return answer.to_text(); }
    }
}
```

The body `contain` takes is a function, so `return` gives the body its value. The body of a `provide` is the same: `return` there gives the provide its value, and does not leave the function.

A failure the caller is not expected to deal with is thrown. `fail` throws an error of a kind, `catch` gives what a function gives or the error it threw, and an error nothing catches ends the program with its trace.

```ab
use ambient.{catch, fail, throw, Error};

tag too_big;

fn small(n: int): int with throw {
    if n > 9 { fail(too_big, "${n} is not small"); }
    return n;
}

pub fn main(): string {
    match catch(fn() => small(12)) {
        let error: Error => { return error.message; }
        let n: int => { return n.to_text(); }
    }
}
```

A function that can fail gives a union, and the caller that cannot go on returns. `let ... else` says so without nesting: the rest of the block is where the value fits, and the arms are where it does not.

```ab
use system.fs;
use ambient.Error;

fn first_line(path: string): string with fs.read {
    let text: string = fs.read!(path) else {
        let error => { return "unreadable: ${error.message}"; }
    };
    let line: string = text.lines().first() else {
        null => { return "empty"; }
    };
    return line;
}

pub fn main(): string {
    let files = { [fs.read](path) => "one\ntwo" };
    return provide files => first_line("notes.txt");
}
```

## Doing several things

Tasks are structured: each finishes or is cancelled before the construct that started it gives a value.

```ab
use ambient.tasks;

fn slow(n: int): int => n * n;

pub fn main(): int {
    let (a, b) = tasks.all(fn() => slow(3), fn() => slow(4));
    return a + b;
}
```

`tasks.first(...)` gives whichever finishes first and cancels the rest. `tasks.group(fn() { ... })` holds tasks started in its body with `tasks.spawn(fn() { ... })`.

Those are written out, one task each. For as many tasks as a list has members, `ambient.tasks` runs a list of functions: `tasks.map` works on every item at once, `tasks.gather` gives what each function of a list gives, and `tasks.race` what the first one does.

```ab
use ambient.tasks;

fn slow(n: int): int => n * n;

pub fn main(): int => tasks.map([1, 2, 3, 4], slow).sum();
```

Whatever waits on the world suspends its task and blocks nothing: sleeping, running a command, reading a line, sending a request. So tasks wait together, and a sleep beside other work in a `tasks.first` is a limit on it. Nothing takes a timeout.

```ab
use ambient.time;
use system.clock;
use system.http;
use ambient.tasks;

fn status(url: string): string with http.send =>
    match http.get(url) {
        let response: http.Response => response.status.to_text(),
        let error: Error => error.message,
    };

fn late(): string with clock.sleep {
    clock.sleep!(time.seconds(2));
    return "gave up";
}

pub fn main(): string with http.send, clock.sleep =>
    tasks.first(fn() => status("http://example.test/"), fn() => late());
```

## Serving

A server is a function from a request to a response. `http.serve` runs it for each request, a number of them at a time, until the server is closed.

```ab
use system.console;
use system.http;

fn answer(request: http.Request): http.Response =>
    http.text(200, "you asked for ${request.url}\n");

pub fn main(): null with http, console.print {
    match http.listen!("127.0.0.1", 8080) {
        let server: http.Server => {
            console.print!("listening on ${server.port}");
            http.serve(server, 16, answer);
        }
        let error: Error => { console.print!(error.message); }
    }
}
```

A body is `bytes`, since not everything sent is text: `request.body.to_text()` is the string it encodes, or null. `http.text` and `http.json` make a response from a string or a JSON value.

`examples/notes` is a whole one, with tests that call its routes with requests as values.

A server that is asked to stop leaves when it chooses. `process.signal` waits to be asked, so serving beside it in a `first` is serving until then:

```ab
use system.console;
use system.http;
use system.process;
use ambient.tasks;

fn answer(request: http.Request): http.Response => http.text(200, "hello\n");

fn served(server: http.Server): string with http {
    http.serve(server, 16, answer);
    return "closed";
}

fn asked(): string with process.signal {
    let _ = process.signal!();
    return "asked to stop";
}

pub fn main(): null with http, process.signal, console.print {
    match http.listen!("127.0.0.1", 8080) {
        let server: http.Server => {
            console.print!(tasks.first(fn() => served(server), fn() => asked()));
            http.close!(server);
        }
        let error: Error => { console.print!(error.message); }
    }
}
```

What is below HTTP is there too. `system.net` connects and listens by TCP and on Unix sockets, and sends datagrams; a connection is a `system.stream.Stream`, read a piece at a time. `http.stream` gives a response whose body is such a stream, for one that does not end; `fs.watch` gives a watcher whose next change a task waits for. Each waits as `http.accept` does, so a `clock.sleep` beside one in a `first` is its timeout.

## Changing a running program

`system.code` packs a function, with everything it reaches, into bytes. `code.compile!` is the compiler: it checks a pack as its source was checked, and gives code for this machine as bytes. `code.load!` adds compiled code to the program that is running, and `code.unpack` does both. What is loaded is `unknown` until a cast says what it must be.

```ab
use system.code;
use system.console;

type Answer = fn(name: string): string;

fn polite(name: string): string => "good day, ${name}";
fn curt(name: string): string => "hi ${name}";

/// Answers with what was packed from now on, if it is a way to answer.
fn deploy(current: Cell<Answer>, packed: bytes): null with code.compile, code.load {
    let loaded = code.unpack(packed);
    let _ = current.update(fn(now: Answer): Answer => cast::<Answer>(loaded, now));
}

pub fn main(): null with code, console.print {
    let current = cell::<Answer>(polite);
    console.print!(current.read()("ada"));
    deploy(current, cast::<bytes>(code.pack!(curt), 0x[]));
    console.print!(current.read()("ada"));
}
```

Here the bytes never leave the program. They are as good read from a file or the body of a request, packed by a program built from other source: the code is checked again where it arrives, and one whose type is not the one the cast asks for is turned away there. A server that reads what answers from a cell, as above, is one that can be changed without stopping; a program made with `ambient build` loads as one that is run does.

Compiled code can be kept, in a file or anywhere bytes go, and loaded later without compiling again. Nothing checks it then, so load only what the program compiled itself; a pack is what to accept from anyone else.

`examples/hot` is a whole one: a server that takes a deploy at `/deploy`, and an entry that packs what the site answers, as it is written now, and sends it. `examples/functions` compiles each function it is posted, keeps it in a file, and calls it at a path named for its hash: loaded for the request and let go. The type it casts to has `throw` for a row, so what it runs can do nothing to the machine.

## Bytes, hashes, and time

`bytes` is a run of bytes that need not be text, written out as `0x[89 50 4E 47]`: a value, a pattern in a match, or a type. A string gives its encoding with `to_bytes()`, and bytes give the string they encode, or null, with `to_text()`. Files and HTTP bodies carry them, and `ambient.base64` and `hex()` write them as text.

Hashing is a function, not an ability: it does nothing to the world. The time is an ability, and what is made of it is not.

```ab
use ambient.base64;
use ambient.time;
use system.clock;
use system.crypto;

pub fn main(): string with clock.now {
    let signed = crypto.hmac(crypto.sha256, "key".to_bytes(), "message".to_bytes());
    let day = time.date(clock.now!());
    return "${base64.encode(signed)} in ${day.year}, written ${time.write(time.days(1))}";
}
```

## Testing

A test is a function that takes nothing and whose name begins with `test_`. `ambient test` runs each one and reports everything in it that does not hold. `check` takes whether something holds, and `equal` two values, which it reports when they differ. Both are functions over one ability, `unmet`, which is what a test's row names.

```ab
use system.console;
use ambient.test.{equal, unmet};

fn greet(name: string): null with console.print {
    console.print!("hello, ${name}");
}

fn test_greets_by_name(): null with unmet {
    let said = cell::<string>("");
    provide { [console.print](text) { said.update(fn(_) => text); } } {
        greet("you");
    }
    equal(said.read(), "hello, you", "it greets by name");
}

pub fn main(): null with console.print {
    greet("world");
}
```

`ambient test --only ledger` runs the tests whose module path and name hold `ledger`.

A test is given no world: it provides what the code under test needs, as above, so it says exactly what the world held.

## The standard library

Import a module by path; its public definitions are reached through its name, or taken by name in braces. Two packages come with the compiler: `ambient` is what runs anywhere, and `system` is what a program on a machine can reach.

| Module                                                       | What it has                                                                                                                                             |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ambient.string`, `list`, `int`, `float`, `iterator`, `cell` | the methods of each shape                                                                                                                               |
| `ambient.map`                                                | `Map<V>`: values by string key, read in key order                                                                                                       |
| `ambient.url`                                                | `Url`, `parse`, `write`, `query`, `write_query`, `encode`, `decode`                                                                                     |
| `ambient.json`                                               | `Json`, `parse`, `write`, `get`, `at`                                                                                                                   |
| `ambient.bytes`                                              | the methods of `bytes`, and `of`, `from_hex`                                                                                                            |
| `ambient.base64`                                             | `encode`, `encode_url`, `decode`                                                                                                                        |
| `ambient.time`                                               | `Date`, `date`, `moment`, `weekday`, `write`, `read`, `seconds`, `minutes`, `hours`, `days`                                                             |
| `ambient.tasks`                                              | `map`, `gather`, `race`                                                                                                                                 |
| `system.console`                                             | `write`, `print`, `warn`, `read_line`                                                                                                                   |
| `system.env`                                                 | `args`, `variable`, `variables`, `directory`                                                                                                            |
| `system.fs`                                                  | `read`, `write`, `append`, and each with `_bytes`; `list`, `exists`, `info`, `remove`, `make_directory`, `rename`, `copy`; `watch`, `change`, `unwatch` |
| `system.path`                                                | `parts`, `join`, `name`, `parent`, `extension`, `stem`, `is_absolute`                                                                                   |
| `system.clock`                                               | `now`, `elapsed`, `sleep`                                                                                                                               |
| `system.process`                                             | `exit`, `run`, `command`, `signal`                                                                                                                      |
| `system.http`                                                | `send`, `get`, `post`, `post_json`, `open`, `stream`, `send_trusting`, `listen`, `serve`, `accept`, `respond`, `close`, `text`, `json`                  |
| `system.stream`                                              | `Stream`, `read`, `write`, `close`, `read_all`, `write_text`                                                                                            |
| `system.net`                                                 | `connect`, `connect_unix`, `listen`, `listen_unix`, `accept`, `bind`, `join`, `send_to`, `receive`, `close`                                             |
| `system.code`                                                | `pack`, `compile`, `load`, `unpack`                                                                                                                     |
| `system.random`                                              | `bits`, `between`, `fraction`, `chance`, `pick`, `shuffle`, `bytes`, `seeded`                                                                           |
| `system.crypto`                                              | `hash`, `hmac`, `same`, and the tags `sha256`, `sha512`, `sha1`                                                                                         |
| `ambient.cleanup`                                            | the `dispose` key a `using` reads                                                                                                                       |

```ab
use ambient.map;

pub fn main(): string {
    let stock = map.of([("tea", 3), ("milk", 1)]);
    let more = stock.update("tea", fn(have) => (have ?? 0) + 2);
    return "${more.keys().join_with(", ")}: ${more.get("tea") ?? 0}";
}
```

JSON is read into ordinary values: a list is a `List<Json>`, an object a `Map<Json>`. Text that is not JSON throws.

```ab
use ambient.json;
use ambient.{catch, Error};

pub fn main(): string {
    let read = catch(fn() => json.parse("{\"name\": \"web\", \"ports\": [80, 443]}"));
    match read {
        let error: Error => { return error.message; }
        let config => {
            let port = cast::<int>(json.at(json.get(config, "ports"), 1), 0);
            return "${cast::<string>(json.get(config, "name"), "?")} on ${port}: ${json.write(config)}";
        }
    }
}
```

## What will trip you up

- A type error is reported at its line and column, and so is each function a fault or an uncaught error left: `in app:main.divide at src/main.ab:2:39`. A frame inside the standard library has a name and no place.
- A record type that names a contract says its type again: `{ [dispose](): null }`.
- A record with methods, such as a `Map`, can be tested for only where the value's type leaves no doubt: a `Json` can be cast to `Map<Json>`, an `unknown` cannot.
- Only a function that captured nothing is packed: one written at the top of a module, not one written inside another that uses what is around it.
- A function written inside another may call itself, and then says its parameter and return types.
- A package is depended on by where it is: at a path (`text = { path = "../text" }`, which `ambient add ../text` writes), or in a git repository: `text = { git = "…", branch = "main" }`, or a `tag`, or a whole commit as `rev`. Neither says an ID: the package's own manifest does. `ambient.lock` holds the commit that led to and the ID of the package there, and only `ambient update` reads the branch again. A member of the workspace may also be named by its ID and a version: `text = { id = "…", version = "1.0.0" }`. There are no releases to depend on yet.
- A package fetched from somewhere takes only what its declaration grants it, and nothing if that says nothing: `http = { git = "…", grants = ["system.net"] }`, each an ability or a module of them, as in a row. That holds what its functions take as a program runs and what its comptime calls and tools take as it is built, and what it grants its own dependencies is cut to it. The error says what to add and where. A package of the workspace takes anything.
- Native code is no ability, so it is allowed apart: `native = true`. Nothing holds native code to a row, so that trusts the package with the machine.
- A workspace is a manifest that lists its members under `[workspace]`: `members = ["app", "libs/*"]`, where `*` is every directory there that holds a package. A command run there, where there is no package, is for every member; `-p text` names the one it is for from anywhere in the workspace, as in `ambient run -p app` or `ambient add text -p app`.
