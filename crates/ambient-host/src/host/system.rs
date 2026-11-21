//! The world of a real process.

use std::ffi::OsString;
use std::io::{self, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::linked::Linked;
use super::pending::Pending;
use super::{
    Done, Entry, Failure, Held, Info, Kind, Listening, Open, Opened, Response, Work, World,
};

/// The process the compiler is running in: its streams, files,
/// environment, and clocks.
pub struct System {
    args: Vec<String>,
    started: Instant,
    /// The work it has begun and not yet given the outcome of.
    pending: Pending,
}

impl System {
    /// The world of this process, started with `args`: one that was
    /// linked for no one program, and so can do all a program may ask.
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self::linked(args, &Linked::whole())
    }

    /// The world of a process that was linked with `linked`, started
    /// with `args`. Nothing here names a group a link may leave out.
    #[must_use]
    pub fn linked(args: Vec<String>, linked: &Linked) -> Self {
        Self {
            args,
            started: Instant::now(),
            pending: Pending::new(linked),
        }
    }
}

pub(super) fn failure(error: &io::Error) -> Failure {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => Kind::NotFound,
        io::ErrorKind::PermissionDenied => Kind::Denied,
        _ => Kind::Failed,
    };
    Failure {
        kind,
        message: error.to_string(),
    }
}

fn saturating(value: u128) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl World for System {
    fn write(&mut self, text: &str) {
        let mut out = io::stdout().lock();
        // A closed stream is not the program's failure to handle.
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }

    fn warn(&mut self, text: &str) {
        let _ = io::stderr().lock().write_all(text.as_bytes());
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn variables(&self) -> Vec<(String, String)> {
        // One that is not text is not the program's to read.
        let text = |(name, value): (OsString, OsString)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        };
        let mut variables: Vec<_> = std::env::vars_os().filter_map(text).collect();
        variables.sort();
        variables
    }

    fn info(&mut self, path: &str) -> Result<Info, Failure> {
        let found = std::fs::metadata(path).map_err(|error| failure(&error))?;
        let kind = if found.is_file() {
            Entry::File
        } else if found.is_dir() {
            Entry::Directory
        } else {
            Entry::Other
        };
        let modified = found.modified().ok();
        let modified = modified.and_then(|at| at.duration_since(UNIX_EPOCH).ok());
        Ok(Info {
            kind,
            size: i64::try_from(found.len()).unwrap_or(i64::MAX),
            modified: saturating(modified.map_or(0, |since| since.as_millis())),
        })
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path);
        let written = file.and_then(|mut file| file.write_all(bytes));
        written.map_err(|error| failure(&error))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), Failure> {
        std::fs::rename(from, to).map_err(|error| failure(&error))
    }

    fn copy(&mut self, from: &str, to: &str) -> Result<(), Failure> {
        let copied = std::fs::copy(from, to).map(|_| ());
        copied.map_err(|error| failure(&error))
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, Failure> {
        std::fs::read(path).map_err(|error| failure(&error))
    }

    fn store(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        std::fs::write(path, bytes).map_err(|error| failure(&error))
    }

    fn store_private(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        super::owned::store(path, bytes).map_err(|error| failure(&error))
    }

    fn install(&mut self, path: &str, bytes: &[u8]) -> Result<(), Failure> {
        self.store(path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let runs = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(path, runs).map_err(|error| failure(&error))?;
        }
        Ok(())
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>, Failure> {
        let entries = std::fs::read_dir(path).map_err(|error| failure(&error))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| failure(&error))?;
            let name = entry.file_name().into_string().map_err(|name| Failure {
                kind: Kind::Invalid,
                message: format!("{path} holds a name that is not text: {}", name.display()),
            })?;
            names.push(name);
        }
        names.sort();
        Ok(names)
    }

    fn exists(&mut self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn remove(&mut self, path: &str) -> Result<(), Failure> {
        let removed = if std::path::Path::new(path).is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        removed.map_err(|error| failure(&error))
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Failure> {
        std::fs::create_dir_all(path).map_err(|error| failure(&error))
    }

    fn directory(&self) -> Option<String> {
        let directory = std::env::current_dir().ok()?;
        directory.into_os_string().into_string().ok()
    }

    fn random(&mut self) -> Result<u64, Failure> {
        let mut bits = [0; 8];
        getrandom::fill(&mut bits).map_err(|error| Failure {
            kind: Kind::Failed,
            message: error.to_string(),
        })?;
        Ok(u64::from_le_bytes(bits))
    }

    fn now(&mut self) -> i64 {
        let since = SystemTime::now().duration_since(UNIX_EPOCH);
        saturating(since.map_or(0, |since| since.as_millis()))
    }

    fn elapsed(&mut self) -> i64 {
        saturating(self.started.elapsed().as_nanos())
    }

    fn offset(&mut self, moment: i64) -> i64 {
        super::zone::offset(moment)
    }

    fn listen(&mut self, host: &str, port: i64) -> Result<Listening, Failure> {
        self.pending.http()?.listen(host, port)
    }

    fn respond(&mut self, incoming: i64, response: Response) -> Result<(), Failure> {
        self.pending.http()?.respond(incoming, response)
    }

    fn close(&mut self, server: i64) {
        self.pending.close(server);
    }

    fn abandon(&mut self, incoming: i64) {
        if let Ok(http) = self.pending.http() {
            http.abandon(incoming);
        }
    }

    fn open(&mut self, what: Open) -> Result<Opened, Failure> {
        self.pending.open(what)
    }

    fn shut(&mut self, held: Held) {
        self.pending.shut(held);
    }

    fn begin(&mut self, ticket: u64, work: Work) {
        self.pending.begin(ticket, work);
    }

    fn next(&mut self) -> Option<(u64, Done)> {
        self.pending.next()
    }

    fn forsake(&mut self, ticket: u64) {
        self.pending.forsake(ticket);
    }

    fn attend(&mut self, posts: std::sync::Arc<crate::native::Posts>) {
        self.pending.attend(posts);
    }
}
