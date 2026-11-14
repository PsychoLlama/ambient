//! The world of a real process.

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Failure, Kind, World};

/// The process the compiler is running in: its streams, files,
/// environment, and clocks.
pub struct System {
    args: Vec<String>,
    started: Instant,
}

impl System {
    /// The world of this process, started with `args`.
    #[must_use]
    pub fn new(args: Vec<String>) -> Self {
        Self {
            args,
            started: Instant::now(),
        }
    }
}

fn failure(error: &io::Error) -> Failure {
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

    fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        let read = io::stdin().lock().read_line(&mut line).ok()?;
        if read == 0 {
            return None;
        }
        let ending = line.trim_end_matches(['\n', '\r']).len();
        line.truncate(ending);
        Some(line)
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

    fn now(&mut self) -> i64 {
        let since = SystemTime::now().duration_since(UNIX_EPOCH);
        saturating(since.map_or(0, |since| since.as_millis()))
    }

    fn elapsed(&mut self) -> i64 {
        saturating(self.started.elapsed().as_nanos())
    }

    fn sleep(&mut self, milliseconds: i64) {
        let milliseconds = u64::try_from(milliseconds).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(milliseconds));
    }
}
