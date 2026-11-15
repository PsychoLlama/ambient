//! The world of a real process.

use std::io::{self, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::pending::Pending;
use super::{Done, Failure, Kind, Work, World};

/// The process the compiler is running in: its streams, files,
/// environment, and clocks.
pub struct System {
    args: Vec<String>,
    started: Instant,
    /// The work it has begun and not yet given the outcome of.
    pending: Pending,
}

impl System {
    /// The world of this process, started with `args`.
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self {
            args,
            started: Instant::now(),
            pending: Pending::default(),
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

    fn read(&mut self, path: &str) -> Result<String, Failure> {
        std::fs::read_to_string(path).map_err(|error| failure(&error))
    }

    fn store(&mut self, path: &str, text: &str) -> Result<(), Failure> {
        std::fs::write(path, text).map_err(|error| failure(&error))
    }

    fn list(&mut self, path: &str) -> Result<Vec<String>, Failure> {
        let entries = std::fs::read_dir(path).map_err(|error| failure(&error))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| failure(&error))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
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
        Some(directory.to_string_lossy().into_owned())
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

    fn begin(&mut self, ticket: u64, work: Work) {
        self.pending.begin(ticket, work);
    }

    fn next(&mut self) -> Option<(u64, Done)> {
        self.pending.next()
    }

    fn forsake(&mut self, ticket: u64) {
        self.pending.forsake(ticket);
    }
}
