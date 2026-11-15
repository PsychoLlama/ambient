# Writing a program in Ambient

This is a tour for someone who wants to write and run something. The [spec](../spec/README.md) says what the language is and why; the packages under [`examples/`](../examples) are complete programs.

Every block marked `ab` on this page is a whole module, and a test checks each one.

## A package

A package is a directory with a manifest and its modules under `src/`:

```
hello/
  ambient.toml      its name and its ID
  src/
    main.ab         the module `main`
    text/trim.ab    the module `text.trim`
    lib.ab          what other packages see, if any do
```

```ab
import system.console;

pub fn main(): null with console.print {
    console.print!("hello, world");
}
```

```
ambient init hello                 # makes a package that runs and has a test
ambient run hello                  # runs main.main
ambient run hello -- one two       # what follows `--` is what env.args gives
ambient run hello --entry tools.fmt
ambient check hello                # checks every definition, runs nothing
ambient test hello                 # runs every function named test_*
```

A `main` that gives an integer exits with it. What is built is kept by its content in one store for every package on the machine, so a second run compiles only what changed.

## Functions and values

A body is a block of statements, or `=>` and one expression. A block has no value of its own: `return` gives one, and it is always written.

```ab
fn double(n: int): int => n * 2;

fn describe(n: int): string {
    let twice = double(n);
    if twice > 10 {
        return "big";
    }
    return "small: ${twice}";
}

pub fn main(): string => describe(3);
```

- The values are `null`, booleans, integers, floats, strings, tags, tuples `(1, "a")`, lists `[1, 2]`, records `{ name: "tea", price: 450 }`, and functions.
- Nothing is changed in place. `list.push(x)` gives another list; a record is copied with `{ ...old, price: 500 }`.
- `++` joins strings and lists, and `"${count} left"` writes a value into a string by its `to_text`. Arithmetic on integers faults on overflow and division by zero instead of wrapping.
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

pub fn main(): string => first_word("tea for two") ++ area({ kind: circle, radius: 1.0 }).to_text();
```

- In a pattern a bare name is a type or a tag to test for, and `let name` binds. After `let` in a statement, bare names bind: `let (a, b) = pair;`.
- A match used as a value has expressions for arms; used as a statement it has blocks.
- Absence is `null` in a union (`string | null`), and there is no exception to catch: an operation that can fail gives a union with `Error`.
- `value as int else 0` narrows by a type and says what to give otherwise. It binds loosest of all, so parenthesise it inside a larger expression.
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
import system.console;
import system.fs;
import ambient.Error;

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
import system.console;

fn greet(name: string): null with console.print {
    console.print!("hello, ${name}");
}

/// What `greet` prints, without printing it.
pub fn main(): string {
    let said = cell<string>("");
    let quiet = {
        [console.print](text) { said.update(fn(so_far) => so_far ++ text); },
    };
    provide quiet {
        greet("you");
    }
    return said.read();
}
```

A cell is the one thing that changes: `cell<T>(value)` makes one, with `read`, `update`, and `replace`.

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

- A fault (dividing by zero, overflow, running out of a budget) unwinds. `try` stops it and gives an `Error` value instead, with `kind`, `message`, and `trace`.
- `budget(steps: n, allocation: n)` limits what its body may do, and gives the limit that ran out as a tag.
- `using name = value;` binds a value that carries its own cleanup, which runs when the block ends however it ends.

```ab
import ambient.cleanup.{dispose};
import system.console;
import ambient.Error;

fn risky(by: int): int => 100 / by;

fn noisy(name: string): { [dispose](): null } with console.print =>
    { [dispose]() { console.print!("closed ${name}"); } };

pub fn main(): string with console.print {
    let outcome = try {
        using _ = noisy("the file");
        return risky(0);
    };
    match outcome {
        let error: Error => { return "failed: ${error.message}"; }
        let answer: int => { return answer.to_text(); }
    }
}
```

Inside `try`, `budget`, `provide`, and the other scopes, `return` gives that scope its value; it does not leave the function.

A failure the caller is not expected to deal with is thrown. `fail` throws an error of a kind, `catch` gives what a function gives or the error it threw, and an error nothing catches ends the program with its trace.

```ab
import ambient.{catch, fail, throw, Error};

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
import system.fs;
import ambient.Error;

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
fn slow(n: int): int => n * n;

pub fn main(): int {
    let (a, b) = all(fn() => slow(3), fn() => slow(4));
    return a + b;
}
```

`first(...)` gives whichever finishes first and cancels the rest. A `group { ... }` holds tasks started with `spawn`.

## Testing

A test is a function that takes nothing and whose name begins with `test_`. `ambient test` runs each one and reports every `check` that does not hold.

```ab
import system.console;
import ambient.test.{check};

fn greet(name: string): null with console.print {
    console.print!("hello, ${name}");
}

fn test_greets_by_name(): null with check {
    let said = cell<string>("");
    provide { [console.print](text) { said.update(fn(_) => text); } } {
        greet("you");
    }
    check!(said.read() == "hello, you", "it greets by name");
}

pub fn main(): null with console.print {
    greet("world");
}
```

`ambient test --only ledger` runs the tests whose module path and name hold `ledger`.

A test is given no world: it provides what the code under test needs, as above, so it says exactly what the world held.

## The standard library

Import a module by path; its public definitions are reached through its name, or taken by name in braces. Two packages come with the compiler: `ambient` is what runs anywhere, and `system` is what a program on a machine can reach.

| Module                                                       | What it has                                                                                       |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- |
| `ambient.string`, `list`, `int`, `float`, `iterator`, `cell` | the methods of each shape                                                                         |
| `ambient.map`                                                | `Map<V>`: values by string key, read in key order                                                 |
| `ambient.json`                                               | `Json`, `parse`, `write`, `get`, `at`                                                             |
| `system.console`                                             | `write`, `print`, `warn`, `read_line`                                                             |
| `system.env`                                                 | `args`, `variable`, `variables`, `directory`                                                      |
| `system.fs`                                                  | `read`, `write`, `append`, `list`, `exists`, `info`, `remove`, `make_directory`, `rename`, `copy` |
| `system.path`                                                | `parts`, `join`, `name`, `parent`, `extension`, `stem`, `is_absolute`                             |
| `system.clock`                                               | `now`, `elapsed`, `sleep`                                                                         |
| `system.process`                                             | `exit`, `run`                                                                                     |
| `system.http`                                                | `send`, `get`, `post`, `listen`, `serve`, `accept`, `respond`, `close`, `text`, `json`            |
| `system.random`                                              | `bits`, `between`, `fraction`, `chance`, `pick`, `shuffle`, `seeded`                              |
| `ambient.cleanup`                                            | the `dispose` key a `using` reads                                                                 |

```ab
import ambient.map;

pub fn main(): string {
    let stock = map.of([("tea", 3), ("milk", 1)]);
    let more = stock.update("tea", fn(have) => (have as int else 0) + 2);
    return "${more.keys().join_with(", ")}: ${more.get("tea") as int else 0}";
}
```

JSON is read into ordinary values: a list is a `List<Json>`, an object a `Map<Json>`. Text that is not JSON throws.

```ab
import ambient.json;
import ambient.{catch, Error};

pub fn main(): string {
    let read = catch(fn() => json.parse("{\"name\": \"web\", \"ports\": [80, 443]}"));
    match read {
        let error: Error => { return error.message; }
        let config => {
            let port = json.at(json.get(config, "ports"), 1) as int else 0;
            return "${json.get(config, "name") as string else "?"} on ${port}: ${json.write(config)}";
        }
    }
}
```

## What will trip you up

- A type error is reported at its line and column, and so is each function a fault or an uncaught error left: `in app:main.divide at src/main.ab:2:39`. A frame inside the standard library has a name and no place.
- `all` and `first` followed by `(` always start tasks, so a function cannot be called by those names.
- A record type that names a key says its type again: `{ [dispose](): null }`.
- A record with methods, such as a `Map`, can be tested for only where the value's type leaves no doubt: a `Json` can be asked `as Map<Json>`, an `unknown` cannot.
- A function written inside another may call itself, and then says its parameter and return types.
- There is no way yet to depend on a package except by a path in the manifest: `[dependencies] text = { path = "../text" }`.
