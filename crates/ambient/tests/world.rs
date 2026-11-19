//! The command-line target, over a world that is scripted: what a
//! program reads, writes, and is told. See `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::host::{Command, Ran};
use ambient::value::Value;
use support::Outcome;
use support::scripted::Scripted;

/// Runs `main` under the command-line target in `world`, on every
/// backend, and gives what it gave and the world it left.
fn run(source: &str, world: &Scripted) -> (Outcome, Scripted) {
    world.program(source)
}

#[test]
fn a_program_reads_its_input_as_bytes() {
    let source = r#"
        use system.console;

        fn rest(read: bytes): bytes with console.read =>
            match console.read!(4) {
                null => read,
                let more: bytes => rest(read ++ more),
            };

        fn main(): string with console.read, console.read_line {
            let first = console.read_line!() as string else "";
            return first ++ ":" ++ rest("".to_bytes()).hex();
        }
    "#;
    let mut world = Scripted::default();
    world.input.extend(["one", "abcde"].map(str::to_owned));

    let (outcome, _) = run(source, &world);
    assert_eq!(outcome, Ok(Value::String("one:61626364650a".to_owned())));
}

#[test]
fn a_program_reads_and_writes_its_streams() {
    let source = r#"
        use system.console;

        fn main(): null with console.print, console.read_line, console.warn, console.write {
            console.write!("name? ");
            match console.read_line!() {
                null => { console.warn!("no input"); }
                let name: string => { console.print!("hello, " ++ name); }
            }
        }
    "#;
    let mut world = Scripted::default();
    world.input.push_back("Ada".to_owned());

    let (outcome, after) = run(source, &world);
    assert_eq!(outcome, Ok(Value::Null));
    assert_eq!(after.out, "name? hello, Ada\n");

    let (_, silent) = run(source, &Scripted::default());
    assert_eq!(silent.err, "no input\n");
}

#[test]
fn a_file_that_is_missing_is_an_error_a_program_matches_on() {
    let source = r#"
        use system.fs;
        use ambient.Error;

        fn main(): string with fs.read, fs.write {
            fs.write!("notes.txt", "kept");
            let found = fs.read!("notes.txt") as string else "";
            match fs.read!("missing.txt") {
                let text: string => { return text; }
                let error: Error => {
                    match error.kind {
                        fs.not_found => { return found ++ ": " ++ error.message; }
                        _ => { return "other"; }
                    }
                }
            }
        }
    "#;

    let (outcome, after) = run(source, &Scripted::default());
    assert_eq!(
        outcome,
        Ok(Value::String("kept: no file at missing.txt".to_owned()))
    );
    assert_eq!(after.file("notes.txt"), "kept");
}

#[test]
fn a_file_is_taken_away_once() {
    let source = r#"
        use system.fs;
        use ambient.Error;

        fn main(): string with fs {
            fs.make_directory!("notes");
            fs.write!("notes/a.txt", "kept");
            let first = fs.remove!("notes/a.txt");
            match (first, fs.remove!("notes/a.txt"), fs.exists!("notes/a.txt")) {
                (null, let error: Error, false) => { return error.message; }
                _ => { return "still there"; }
            }
        }
    "#;

    let (outcome, after) = run(source, &Scripted::default());
    assert_eq!(
        outcome,
        Ok(Value::String("no file at notes/a.txt".to_owned()))
    );
    assert!(after.files.is_empty());
}

#[test]
fn a_command_is_run_and_what_it_gave_is_read() {
    let source = r#"
        use system.process;
        use system.process.{Output};
        use ambient.Error;

        fn ran(command: string): string with process.run {
            match process.run!(process.command(command, ["status", "--short"])) {
                let error: Error => {
                    match error.kind {
                        process.not_found => { return "none: " ++ error.message; }
                        _ => { return error.message; }
                    }
                }
                let output: Output => {
                    let out = output.out.to_text() as string else "not text";
                    return "${output.code}: ${out.trim()}";
                }
            }
        }
        fn main(): string with process.run => ran("git") ++ ", " ++ ran("hg");
    "#;
    let gave = Ran {
        code: 1,
        out: b" M notes\n".to_vec(),
        ..Ran::default()
    };
    let world = Scripted {
        commands: [("git status --short".to_owned(), gave)].into(),
        ..Scripted::default()
    };

    assert_eq!(
        run(source, &world).0,
        Ok(Value::String("1: M notes, none: no command hg".to_owned()))
    );
}

#[test]
fn a_command_is_told_where_to_run_and_what_to_read() {
    let source = r#"
        use system.process;

        fn main(): int with process.run {
            let sort = {
                ...process.command("sort", []),
                directory: "/tmp",
                variables: [("LC_ALL", "C")],
                input: "b\na\n".to_bytes(),
            };
            match process.run!(sort) {
                let output: process.Output => { return output.code; }
                _ => { return -2; }
            }
        }
    "#;
    let world = Scripted {
        commands: [("sort".to_owned(), Ran::default())].into(),
        ..Scripted::default()
    };

    let (outcome, after) = run(source, &world);
    let asked = Command {
        program: "sort".to_owned(),
        args: Vec::new(),
        directory: Some("/tmp".to_owned()),
        variables: vec![("LC_ALL".to_owned(), "C".to_owned())],
        input: b"b\na\n".to_vec(),
    };
    assert_eq!((outcome, after.ran), (Ok(Value::Int(0)), vec![asked]));
}

#[test]
fn a_program_is_told_how_it_was_started_and_what_time_it_is() {
    let source = r#"
        use ambient.time;
        use system.clock;
        use system.env;

        fn main(): int with env.args, env.variable, clock.now, clock.sleep {
            let before = clock.now!();
            clock.sleep!(time.milliseconds(env.args!().length() * 10));
            let home = env.variable!("HOME") as string else "";
            return clock.now!() - before + home.length();
        }
    "#;
    let world = Scripted {
        args: vec!["a".to_owned(), "b".to_owned()],
        ..Scripted::default()
    };

    assert_eq!(run(source, &world).0, Ok(Value::Int(20 + 8)));
}

#[test]
fn exiting_ends_the_program_with_its_code() {
    let source = r#"
        use system.console;
        use system.process;

        fn main(): int with console.print, process.exit {
            console.print!("before");
            process.exit!(3);
            console.print!("after");
            return 0;
        }
    "#;

    let (outcome, after) = run(source, &Scripted::default());
    assert_eq!(outcome, Ok(Value::Int(3)));
    assert_eq!(after.out, "before\n");
}

// Waiting

#[test]
fn tasks_that_sleep_do_so_together() {
    let source = r"
        use ambient.time;
        use system.clock;
        use ambient.tasks;

        fn nap(length: int): int with clock.sleep {
            clock.sleep!(time.milliseconds(length));
            return length;
        }

        fn main(): int with clock.sleep, clock.now {
            let (a, b) = tasks.all(fn() => nap(30), fn() => nap(50));
            return clock.now!() * 1000 + a + b;
        }
    ";

    // Fifty milliseconds went by, not eighty.
    let (outcome, _) = run(source, &Scripted::default());
    assert_eq!(outcome, Ok(Value::Int(50_080)));
}

#[test]
fn a_sleep_is_a_limit_on_how_long_to_wait() {
    let source = r#"
        use ambient.time;
        use system.clock;
        use system.console;
        use ambient.tasks;

        fn late(): string with clock.sleep {
            clock.sleep!(time.milliseconds(100));
            return "too late";
        }

        fn main(): string with clock.sleep, console.read_line {
            return tasks.first(
                fn() => console.read_line!() as string else "nothing",
                fn() => late(),
            );
        }
    "#;
    let world = Scripted {
        input: ["typed".to_owned()].into(),
        ..Scripted::default()
    };

    // The sleep that lost was stopped, and no time went by.
    let (outcome, after) = run(source, &world);
    assert_eq!(outcome, Ok(Value::String("typed".to_owned())));
    assert_eq!((after.time, after.forsaken.len()), (0, 1));
}

#[test]
fn a_program_draws_numbers_from_the_world() {
    let source = r"
        use system.random;

        fn main(): int with random.bits => random.between(10, 20) * 100 + random.between(0, 5);
    ";

    // The scripted world's bits count up from one.
    let (outcome, after) = run(source, &Scripted::default());
    assert_eq!((outcome, after.rolled), (Ok(Value::Int(1102)), 2));
}

// Files

#[test]
fn a_file_is_looked_at_added_to_copied_and_moved() {
    let source = r#"
        use system.fs;

        fn size(path: string): string with fs.info {
            match fs.info!(path) {
                let info: fs.Info => {
                    match info.kind {
                        fs.file => { return "file of ${info.size}"; }
                        fs.directory => { return "directory"; }
                        _ => { return "other"; }
                    }
                }
                let error: Error => { return error.message; }
            }
        }

        fn main(): string with fs {
            let _ = fs.append!("logs/today", "one\n");
            let _ = fs.append!("logs/today", "two\n");
            let _ = fs.copy!("logs/today", "logs/kept");
            let _ = fs.rename!("logs/today", "logs/old");
            return [size("logs/kept"), size("logs"), size("logs/today")].join_with(", ");
        }
    "#;

    let (outcome, after) = run(source, &Scripted::default());
    let expected = "file of 8, directory, no file at logs/today";
    assert_eq!(outcome, Ok(Value::String(expected.to_owned())));
    assert_eq!(after.file("logs/old"), "one\ntwo\n");
}

#[test]
fn a_program_reads_its_whole_environment() {
    let source = r#"
        use system.env;

        fn main(): string with env {
            let named = fn(variable: (string, string)): string {
                let (name, value) = variable;
                return "${name}=${value}";
            };
            let directory = env.directory!() as string else "nowhere";
            return env.variables!().map(named).push(directory).join_with(" ");
        }
    "#;
    let world = Scripted {
        directory: Some("/work".to_owned()),
        ..Scripted::default()
    };

    let expected = "HOME=/home/me /work";
    assert_eq!(
        run(source, &world).0,
        Ok(Value::String(expected.to_owned()))
    );
}

#[test]
fn a_file_that_is_not_text_is_read_and_written_as_bytes() {
    let source = r#"
        use system.fs;
        use ambient.bytes;

        fn main(): string with fs {
            let image = bytes.of([137, 80, 78, 71]) as bytes else "".to_bytes();
            let _ = fs.write_bytes!("logo.png", image);
            let _ = fs.append_bytes!("logo.png", "!".to_bytes());
            let held = fs.read_bytes!("logo.png") as bytes else "".to_bytes();
            match fs.read!("logo.png") {
                let text: string => { return text; }
                let error: Error => {
                    let invalid = error.kind == fs.invalid;
                    return "${held.hex()} ${invalid}: ${error.message}";
                }
            }
        }
    "#;

    let (outcome, after) = run(source, &Scripted::default());
    let expected = "89504e4721 true: the file at logo.png is not text";
    assert_eq!(outcome, Ok(Value::String(expected.to_owned())));
    assert_eq!(after.files["logo.png"], [137, 80, 78, 71, b'!']);
}
