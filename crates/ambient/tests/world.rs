//! The command-line target, over a world that is scripted: what a
//! program reads, writes, and is told. See `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use ambient::engine::Engine;
use ambient::host::{self, Failure, Kind, World};
use ambient::store::MemoryStore;
use ambient::value::Value;
use support::{Outcome, run_entry, value};

/// A world held in memory.
#[derive(Clone, Default)]
struct Scripted {
    out: String,
    err: String,
    input: VecDeque<String>,
    files: BTreeMap<String, String>,
    args: Vec<String>,
    /// What the clock says; sleeping moves it on.
    time: i64,
}

impl World for Scripted {
    fn write(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn warn(&mut self, text: &str) {
        self.err.push_str(text);
    }

    fn read_line(&mut self) -> Option<String> {
        self.input.pop_front()
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn variable(&self, name: &str) -> Option<String> {
        (name == "HOME").then(|| "/home/me".to_owned())
    }

    fn read(&mut self, path: &str) -> Result<String, Failure> {
        self.files.get(path).cloned().ok_or_else(|| Failure {
            kind: Kind::NotFound,
            message: format!("no file at {path}"),
        })
    }

    fn store(&mut self, path: &str, text: &str) -> Result<(), Failure> {
        self.files.insert(path.to_owned(), text.to_owned());
        Ok(())
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>, Failure> {
        let inside = |file: &&String| file.starts_with(path);
        Ok(self.files.keys().filter(inside).cloned().collect())
    }

    fn exists(&mut self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn now(&mut self) -> i64 {
        self.time
    }

    fn elapsed(&mut self) -> i64 {
        self.time * 1_000_000
    }

    fn sleep(&mut self, milliseconds: i64) {
        self.time += milliseconds;
    }
}

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
