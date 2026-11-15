//! A world held in memory, for programs that reach one.

use std::collections::{BTreeMap, VecDeque};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use ambient::cli::{Command, Session};
use ambient::host::{
    Done, Failure, Incoming, Kind, Listening, Ran, Request, Response, Work, World,
};
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
