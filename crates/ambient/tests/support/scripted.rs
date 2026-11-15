//! A world held in memory, for programs that reach one.

use std::collections::{BTreeMap, VecDeque};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::engine::Engine;
use ambient::host::{
    Done, Entry, Failure, Incoming, Info, Kind, Listening, Ran, Request, Response, Work, World,
};
use ambient::project::Files;
use ambient::store::MemoryStore;

use super::Outcome;

/// A world held in memory.
#[derive(Clone, Default)]
pub struct Scripted {
    pub out: String,
    pub err: String,
    pub input: VecDeque<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub args: Vec<String>,
    /// What each command gives, by the command and its arguments with
    /// a space between.
    pub commands: BTreeMap<String, Ran>,
    /// Where a path that does not say where it starts is.
    pub directory: Option<String>,
    /// How many random numbers were asked for; the next is one more.
    pub rolled: u64,
    /// What the clock says. Waiting for work moves it on to when the
    /// work is over.
    pub time: i64,
    /// The work under way: when it is over, its ticket, and what
    /// comes of it.
    pub waits: Vec<(i64, u64, Done)>,
    /// The tickets of the work that was stopped, in order.
    pub forsaken: Vec<u64>,
    /// What each request gets, by its method and URL with a space
    /// between; any other reaches nothing.
    pub responses: BTreeMap<String, Response>,
    /// Every request that was sent.
    pub sent: Vec<Request>,
    /// The requests a server is sent. It is closed once it has given
    /// them all.
    pub requests: VecDeque<Request>,
    /// Every answer a server gave, in order.
    pub answers: Vec<Response>,
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
        ("app/ambient.toml", "name = \"app\"\nid = \"app\""),
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

/// Runs `main` of a program written as one module under the
/// command-line target, on every backend, each in a world `make` makes.
pub fn over<W: World + 'static>(source: &str, make: impl Fn() -> Rc<RefCell<W>>) -> Outcome {
    let workspace = super::text::workspace(source);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let target = ambient::system::target();
    let entry = engine
        .program(&super::value("main", "main"), &target)
        .unwrap();
    super::run_entry(&engine, &entry, &[], || {
        let world: Rc<RefCell<dyn World>> = make();
        ambient::host::natives(&world)
    })
}

impl Scripted {
    /// Runs `main` of a program written as one module in this world,
    /// and gives what it gave and the world it left. Each backend runs
    /// in a world of its own, and they must leave the same output.
    pub fn program(&self, source: &str) -> (Outcome, Scripted) {
        let left = RefCell::new(Vec::new());
        let outcome = over(source, || {
            let world = Rc::new(RefCell::new(self.clone()));
            left.borrow_mut().push(Rc::clone(&world));
            world
        });
        let mut worlds = left.into_inner().into_iter();
        let first = worlds.next().unwrap().borrow().clone();
        for other in worlds {
            assert_eq!(other.borrow().out, first.out, "the backends disagree");
        }
        (outcome, first)
    }

    /// The text a file holds.
    pub fn file(&self, path: &str) -> &str {
        std::str::from_utf8(&self.files[path]).unwrap()
    }
}

fn missing(path: &str) -> Failure {
    Failure {
        kind: Kind::NotFound,
        message: format!("no file at {path}"),
    }
}

impl World for Scripted {
    fn write(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn warn(&mut self, text: &str) {
        self.err.push_str(text);
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn variable(&self, name: &str) -> Option<String> {
        (name == "HOME").then(|| "/home/me".to_owned())
    }

    fn variables(&self) -> Vec<(String, String)> {
        vec![("HOME".to_owned(), "/home/me".to_owned())]
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, Failure> {
        self.files.get(path).cloned().ok_or_else(|| missing(path))
    }

    /// A directory is whatever a file is inside.
    fn info(&mut self, path: &str) -> Result<Info, Failure> {
        let inside = format!("{}/", path.trim_end_matches('/'));
        let (kind, size) = match self.files.get(path) {
            Some(text) => (Entry::File, text.len()),
            None if self.files.keys().any(|file| file.starts_with(&inside)) => {
                (Entry::Directory, 0)
            }
            None => return Err(missing(path)),
        };
        Ok(Info {
            kind,
            size: i64::try_from(size).unwrap_or(i64::MAX),
            modified: self.time,
        })
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        self.files
            .entry(path.to_owned())
            .or_default()
            .extend_from_slice(bytes);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Failure> {
        let text = self.files.remove(from).ok_or_else(|| missing(from))?;
        self.files.insert(to.to_owned(), text);
        Ok(())
    }

    fn copy(&mut self, from: &str, to: &str) -> Result<(), Failure> {
        let text = self.read(from)?;
        self.files.insert(to.to_owned(), text);
        Ok(())
    }

    fn store(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        self.files.insert(path.to_owned(), bytes.to_owned());
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
        self.files
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| missing(path))
    }

    /// A file is anywhere it is written, so there is nothing to make.
    fn make_directory(&mut self, _path: &str) -> Result<(), Failure> {
        Ok(())
    }

    fn directory(&self) -> Option<String> {
        self.directory.clone()
    }

    fn random(&mut self) -> Result<u64, Failure> {
        self.rolled += 1;
        Ok(self.rolled)
    }

    fn now(&mut self) -> i64 {
        self.time
    }

    fn elapsed(&mut self) -> i64 {
        self.time * 1_000_000
    }

    fn listen(&mut self, _host: &str, port: i64) -> Result<Listening, Failure> {
        Ok(Listening { id: 1, port })
    }

    fn respond(&mut self, _incoming: i64, response: Response) -> Result<(), Failure> {
        self.answers.push(response);
        Ok(())
    }

    fn close(&mut self, _server: i64) {
        self.requests.clear();
    }

    /// Only a sleep takes time here: a command and a line of input
    /// are over as soon as they are begun.
    fn begin(&mut self, ticket: u64, work: Work) {
        let (takes, done) = match work {
            Work::Sleep(milliseconds) => (milliseconds.max(0), Done::Slept),
            Work::ReadLine => (0, Done::Line(self.input.pop_front())),
            Work::Send(request) => {
                let line = format!("{} {}", request.method, request.url);
                let response = self.responses.get(&line).cloned().ok_or_else(|| Failure {
                    kind: Kind::Unreachable,
                    message: format!("nothing answers {}", request.url),
                });
                self.sent.push(request);
                (0, Done::Sent(response))
            }
            Work::Accept(_) => {
                let request = self.requests.pop_front();
                let id = i64::try_from(self.requests.len()).unwrap_or(0);
                (
                    0,
                    Done::Accepted(request.map(|request| Incoming { id, request })),
                )
            }
            Work::Run { command, args } => {
                let line = [std::slice::from_ref(&command), &args[..]]
                    .concat()
                    .join(" ");
                let ran = self.commands.get(&line).cloned().ok_or_else(|| Failure {
                    kind: Kind::NotFound,
                    message: format!("no command {command}"),
                });
                (0, Done::Ran(ran))
            }
        };
        self.waits.push((self.time + takes, ticket, done));
    }

    fn next(&mut self) -> Option<(u64, Done)> {
        let soonest = (0..self.waits.len()).min_by_key(|index| self.waits[*index].0)?;
        let (over, ticket, done) = self.waits.remove(soonest);
        self.time = self.time.max(over);
        Some((ticket, done))
    }

    fn forsake(&mut self, ticket: u64) {
        self.waits.retain(|wait| wait.1 != ticket);
        self.forsaken.push(ticket);
    }
}
