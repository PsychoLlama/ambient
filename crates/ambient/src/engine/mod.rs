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

mod annotate;
mod asked;
mod blame;
mod check;
mod compile;
mod environment;
mod function;
mod generate;
mod group;
mod handed;
mod link;
mod pack;
mod program;
mod read;
mod resolve;
mod run;
mod scope;
mod verify;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash as StdHash;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Error;
use crate::hash::{Hash, Hasher, encode};
use crate::store::Store;
use crate::symbol::{ModuleId, Symbol};
use crate::workspace::Sources;

pub use ambient_host::parts::{Artifact, Body, Native, Part, Parts, Unloadable};
pub use asked::{Files, refused};
pub use check::Interface;
pub use compile::Backend;
pub use read::{Entry, Read, Table};
pub use resolve::Resolved;
pub use scope::{Member, Members, Scope};

/// The answer to a question: shared, or the problem that prevented it.
pub type Query<T> = Result<Arc<T>, Error>;

/// The build of the compiler this is: its source, the host's, the
/// runtime's, and the versions of what they depend on. Nothing one
/// build stored is reused by another.
const BUILD: &str = env!("AMBIENT_BUILD");

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
    scope: Memo<ModuleId, scope::Scoped>,
    members: Memo<ModuleId, Members>,
    /// The modules whose members are being worked out, outermost first:
    /// how a cycle of re-exports is seen.
    exporting: RefCell<Vec<ModuleId>>,
    resolve: Memo<Symbol, Resolved>,
    interface: Memo<Symbol, Interface>,
    type_info: Memo<Symbol, crate::types::TypeInfo>,
    /// The type each derived definition's call gave.
    derived: Memo<Symbol, crate::types::Ty>,
    /// The function each generated definition's call gave.
    generated: Memo<Symbol, crate::surface::Function<crate::surface::Reference>>,
    /// The definitions each generated module's call gave.
    made: Memo<ModuleId, Vec<crate::surface::Definition<crate::surface::Reference>>>,
    /// The derived definitions whose calls are being run, outermost
    /// first: how one that depends on itself is seen.
    deriving: RefCell<Vec<Symbol>>,
    check: Memo<Symbol, crate::core::Open>,
    /// The node of its body each function that does not check is wrong
    /// at.
    blamed: RefCell<HashMap<Symbol, usize>>,
    /// The node of the function its call gave that each generated
    /// function that does not check is wrong at.
    given_at: RefCell<HashMap<Symbol, usize>>,
    /// The code a comptime call gave that each function that does not
    /// check is wrong in: see [`crate::checker::Noted::given`].
    spliced: RefCell<HashMap<Symbol, String>>,
    /// What each function checked so far was checked from: see
    /// [`crate::checker::Noted::origins`].
    origins: RefCell<HashMap<Symbol, crate::checker::Origins>>,
    /// The definitions packs brought, by the symbols they were given.
    received: RefCell<HashMap<Symbol, Arc<crate::core::Open>>>,
    hash: RefCell<HashMap<Symbol, Hash>>,
    compile: Memo<(Symbol, String, bool), Artifact>,
    /// Whether each function asked about is handed its type arguments.
    handed: RefCell<HashMap<Symbol, bool>>,
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
            members: RefCell::default(),
            exporting: RefCell::default(),
            resolve: RefCell::default(),
            interface: RefCell::default(),
            type_info: RefCell::default(),
            derived: RefCell::default(),
            generated: RefCell::default(),
            made: RefCell::default(),
            deriving: RefCell::default(),
            check: RefCell::default(),
            blamed: RefCell::default(),
            given_at: RefCell::default(),
            spliced: RefCell::default(),
            origins: RefCell::default(),
            received: RefCell::default(),
            hash: RefCell::default(),
            compile: RefCell::default(),
            handed: RefCell::default(),
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
        // The compiler is an input to every stage, and no key covers it:
        // a result is stored under its key and the build that made it.
        let key = kept(&key, BUILD);
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

/// Where the result under `key` is kept by one build of the compiler.
fn kept(key: &Hash, build: &str) -> Hash {
    Hasher::new("stage")
        .hash(key)
        .bytes(build.as_bytes())
        .finish()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_build_keeps_a_result_somewhere_else() {
        let key = Hasher::new("check").finish();

        assert_eq!(kept(&key, "one"), kept(&key, "one"));
        assert_ne!(kept(&key, "one"), kept(&key, "two"));
    }
}
