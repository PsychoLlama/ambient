//! The command-line target, over a world that is scripted: what a
//! program reads, writes, and is told. See `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::engine::Engine;
use ambient::host::{self, Ran, World};
use ambient::store::MemoryStore;
use ambient::value::Value;
use support::scripted::Scripted;
use support::{Outcome, run_entry, value};

/// Runs `main` under the command-line target in `world`, on every
/// backend, and gives what it gave and the world it left.
fn run(source: &str, world: &Scripted) -> (Outcome, Scripted) {
    let workspace = support::text::workspace(source);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let entry = engine
        .program(&value("main", "main"), &host::target())
        .unwrap();
    // Each backend runs in a world of its own; they must agree.
    let left = RefCell::new(Vec::new());
    let natives = || {
        let world: Rc<RefCell<Scripted>> = Rc::new(RefCell::new(world.clone()));
        left.borrow_mut().push(Rc::clone(&world));
        let world: Rc<RefCell<dyn World>> = world;
        host::natives(&world)
    };
    let outcome = run_entry(&engine, &entry, &[], natives);
    let mut worlds = left.into_inner().into_iter();
    let first = worlds.next().unwrap().borrow().clone();
    for other in worlds {
        assert_eq!(other.borrow().out, first.out, "the backends disagree");
    }
    (outcome, first)
}

#[test]
fn a_program_reads_and_writes_its_streams() {
    let source = r#"
        import ambient.console;

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
        import ambient.fs;
        import ambient.Error;

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
    assert_eq!(after.files["notes.txt"], "kept");
}

#[test]
fn a_file_is_taken_away_once() {
    let source = r#"
        import ambient.fs;
        import ambient.Error;

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
        import ambient.process;
        import ambient.process.{Output};
        import ambient.Error;

        fn ran(command: string): string with process.run {
            match process.run!(command, ["status", "--short"]) {
                let error: Error => { return error.message; }
                let output: Output => { return "${output.code}: ${output.out.trim()}"; }
            }
        }
        fn main(): string with process.run => ran("git") ++ ", " ++ ran("hg");
    "#;
    let gave = Ran {
        code: 1,
        out: " M notes\n".to_owned(),
        ..Ran::default()
    };
    let world = Scripted {
        commands: [("git status --short".to_owned(), gave)].into(),
        ..Scripted::default()
    };

    assert_eq!(
        run(source, &world).0,
        Ok(Value::String("1: M notes, no command hg".to_owned()))
    );
}

#[test]
fn a_program_is_told_how_it_was_started_and_what_time_it_is() {
    let source = r#"
        import ambient.clock;
        import ambient.env;

        fn main(): int with env.args, env.variable, clock.now, clock.sleep {
            let before = clock.now!();
            clock.sleep!(env.args!().length() * 10);
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
        import ambient.console;
        import ambient.process;

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
