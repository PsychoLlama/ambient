//! The build engine: demand-driven stages over a store. See `spec/build.md`.
//!
//! An [`Engine`] answers questions about one snapshot of a program. Nothing
//! is computed until a question needs it.
//!
//! Each stage (read, resolve, check, compile) follows the same three steps:
//!
//! 1. Gather the stage's inputs by asking other questions.
//! 2. Derive a key from the content of those inputs.
//! 3. Take the result from the [`Store`] under that key, or compute and
//!    store it.
//!
//! Step 3 must read nothing that step 2 did not put in the key. That rule is
//! what makes a stored result safe to reuse for any workspace, so compute
//! functions take their inputs as arguments and never see the engine.
//!
//! Hashing is not stored: it costs no more than a lookup would.
//!
//! An engine also remembers every answer for its own lifetime, so asking
//! twice costs nothing. A new snapshot needs a new engine over the same
//! store.

mod check;
mod compile;
mod group;
mod link;
mod program;
mod read;
mod resolve;
mod run;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash as StdHash;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Error;
use crate::hash::{Hash, encode};
use crate::store::Store;
use crate::symbol::{ModuleId, Symbol};
use crate::workspace::Sources;

pub use check::Interface;
pub use compile::{Artifact, Backend};
pub use link::{Body, Native, Part, Parts};
pub use read::{Entry, Read, Scope, Table};
pub use resolve::Resolved;

/// The answer to a question: shared, or the problem that prevented it.
pub type Query<T> = Result<Arc<T>, Error>;

/// A stage whose results are kept in the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Resolve,
    Check,
    Compile,
    /// A function called during the build.
    Run,
}

/// Whether a stage did its work or took the result from the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ran,
    Reused,
}

/// One stage reaching the store on behalf of one module or symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub stage: Stage,
    pub subject: String,
    pub outcome: Outcome,
}

type Memo<K, T> = RefCell<HashMap<K, Query<T>>>;

pub struct Engine<'a> {
    sources: &'a dyn Sources,
    store: &'a dyn Store,
    events: RefCell<Vec<Event>>,
    read: Memo<ModuleId, Read>,
    scope: Memo<ModuleId, read::Scoped>,
    resolve: Memo<Symbol, Resolved>,
    interface: Memo<Symbol, Interface>,
    type_info: Memo<Symbol, crate::types::TypeInfo>,
    check: Memo<Symbol, crate::core::Open>,
    hash: RefCell<HashMap<Symbol, Hash>>,
    compile: Memo<(Symbol, String), Artifact>,
    /// The definitions being checked, outermost first. A comptime call
    /// makes checking re-entrant; this is how a cycle through one is seen.
    checking: RefCell<Vec<Symbol>>,
    /// The roots of the programs asked for, each with what it is made of.
    roots: RefCell<HashMap<Symbol, program::Root>>,
    runner: run::Runner,
    /// The world comptime calls run in, if the build has one.
    build: Option<run::Build>,
    /// The runs made while checking each definition being checked,
    /// outermost first.
    observed: RefCell<Vec<Vec<run::Dependency>>>,
}

impl<'a> Engine<'a> {
    #[must_use]
    pub fn new(sources: &'a dyn Sources, store: &'a dyn Store) -> Self {
        Self {
            sources,
            store,
            events: RefCell::default(),
            read: RefCell::default(),
            scope: RefCell::default(),
            resolve: RefCell::default(),
            interface: RefCell::default(),
            type_info: RefCell::default(),
            check: RefCell::default(),
            hash: RefCell::default(),
            compile: RefCell::default(),
            checking: RefCell::default(),
            roots: RefCell::default(),
            runner: run::Runner::default(),
            build: None,
            observed: RefCell::default(),
        }
    }

    /// Every time a stage reached the store, in order.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }

    /// The subjects for which `stage` did its work, sorted.
    #[must_use]
    pub fn ran(&self, stage: Stage) -> Vec<String> {
        self.subjects(stage, Outcome::Ran)
    }

    /// The subjects for which `stage` took its result from the store, sorted.
    #[must_use]
    pub fn reused(&self, stage: Stage) -> Vec<String> {
        self.subjects(stage, Outcome::Reused)
    }

    fn subjects(&self, stage: Stage, outcome: Outcome) -> Vec<String> {
        let mut subjects: Vec<String> = self
            .events
            .borrow()
            .iter()
            .filter(|event| event.stage == stage && event.outcome == outcome)
            .map(|event| event.subject.clone())
            .collect();
        subjects.sort();
        subjects
    }

    /// Step 3 of a stage: the stored result under `key`, or `compute`'s.
    fn stage<T>(
        &self,
        stage: Stage,
        subject: &dyn Display,
        key: Hash,
        compute: impl FnOnce() -> Result<T, Error>,
    ) -> Query<T>
    where
        T: Serialize + DeserializeOwned,
    {
        self.stage_if(stage, subject, key, |_| true, compute)
    }

    /// A stage whose result depends on more than its key covers: the
    /// stored result is taken only if `holds` says it still stands.
    ///
    /// This is for results that observed the world. What they observed is
    /// stored with them, and `holds` looks again.
    fn stage_if<T>(
        &self,
        stage: Stage,
        subject: &dyn Display,
        key: Hash,
        holds: impl FnOnce(&Result<T, Error>) -> bool,
        compute: impl FnOnce() -> Result<T, Error>,
    ) -> Query<T>
    where
        T: Serialize + DeserializeOwned,
    {
        // An entry that does not decode is treated as missing: the store is
        // only ever a cache.
        let stored = self
            .store
            .get(&key)
            .and_then(|bytes| postcard::from_bytes::<Result<T, Error>>(&bytes).ok())
            .filter(holds);
        let (result, outcome) = if let Some(result) = stored {
            (result, Outcome::Reused)
        } else {
            let result = compute();
            self.store.put(key, encode(&result));
            (result, Outcome::Ran)
        };
        self.events.borrow_mut().push(Event {
            stage,
            subject: subject.to_string(),
            outcome,
        });
        result.map(Arc::new)
    }
}

/// The answer already given for `key`, or `compute`'s, remembered.
fn memo<K, T>(memo: &Memo<K, T>, key: &K, compute: impl FnOnce() -> Query<T>) -> Query<T>
where
    K: StdHash + Eq + Clone,
{
    if let Some(answer) = memo.borrow().get(key) {
        return answer.clone();
    }
    let answer = compute();
    memo.borrow_mut().insert(key.clone(), answer.clone());
    answer
}
