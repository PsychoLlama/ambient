//! A world held in memory, for programs that reach one.

use std::collections::{BTreeMap, VecDeque};

use ambient::host::{Failure, Kind, World};

/// A world held in memory.
#[derive(Clone, Default)]
pub struct Scripted {
    pub out: String,
    pub err: String,
    pub input: VecDeque<String>,
    pub files: BTreeMap<String, String>,
    pub args: Vec<String>,
    /// What the clock says; sleeping moves it on.
    pub time: i64,
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
