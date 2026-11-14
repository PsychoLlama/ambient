//! A world held in memory, for programs that reach one.

use std::collections::{BTreeMap, VecDeque};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::host::{Failure, Kind, Ran, World};
use ambient::project::Files;
use ambient::store::MemoryStore;

/// A world held in memory.
#[derive(Clone, Default)]
pub struct Scripted {
    pub out: String,
    pub err: String,
    pub input: VecDeque<String>,
    pub files: BTreeMap<String, String>,
    pub args: Vec<String>,
    /// What each command gives, by the command and its arguments with
    /// a space between.
    pub commands: BTreeMap<String, Ran>,
    /// What the clock says; sleeping moves it on.
    pub time: i64,
}

impl Scripted {
    /// What a command over `files` exits with in this world, and the
    /// world it leaves.
    pub fn execute(self, files: &dyn Files, command: &Command) -> (i32, Self) {
        let scripted = Rc::new(RefCell::new(self));
        let world: Rc<RefCell<dyn World>> = scripted.clone();
        let store = MemoryStore::new();
        let session = Session {
            files,
            store: &store,
            world,
        };
        let code = session.execute(command);
        let after = scripted.borrow().clone();
        (code, after)
    }
}

/// The files of a package `app` whose `main` module is `source`.
pub fn app(source: &str) -> BTreeMap<PathBuf, String> {
    let file = |(path, text): (&str, &str)| (PathBuf::from(path), text.to_owned());
    let files = [
        ("app/ambient.toml", "name = \"app\""),
        ("app/src/main.ab", source),
    ];
    files.into_iter().map(file).collect()
}

/// Checks the package `app` of `files`.
pub fn check(files: &dyn Files) -> (i32, Scripted) {
    let path = PathBuf::from("app");
    Scripted::default().execute(files, &Command::Check { path })
}

/// Runs `main` of module `main` of the package `app` of `files`.
pub fn run(files: &dyn Files) -> (i32, Scripted) {
    let command = Command::Run {
        path: PathBuf::from("app"),
        entry: "main.main".to_owned(),
        args: Vec::new(),
    };
    Scripted::default().execute(files, &command)
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

    fn remove(&mut self, path: &str) -> Result<(), Failure> {
        self.files.remove(path).map(|_| ()).ok_or_else(|| Failure {
            kind: Kind::NotFound,
            message: format!("no file at {path}"),
        })
    }

    /// A file is anywhere it is written, so there is nothing to make.
    fn make_directory(&mut self, _path: &str) -> Result<(), Failure> {
        Ok(())
    }

    fn run(&mut self, command: &str, args: &[String]) -> Result<Ran, Failure> {
        let line = [&[command.to_owned()], args].concat().join(" ");
        self.commands.get(&line).cloned().ok_or_else(|| Failure {
            kind: Kind::NotFound,
            message: format!("no command {command}"),
        })
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
