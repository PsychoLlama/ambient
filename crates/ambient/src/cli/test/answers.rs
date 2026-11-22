//! What the command answers for the program that runs tests, as the
//! externs of `ambient.test.running`: running one test, what is kept of
//! one, and each event it reports, which is written as it comes.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::rc::Rc;

use super::said::{inner, problems};
use crate::backend::native::{Natives, Questions};
use crate::cli::{Launch, trace};
use crate::engine::{definition, refused};
use crate::error::Error;
use crate::host::World;
use crate::symbol::Symbol;
use crate::value::Value;

pub(super) struct Answers<'a> {
    launch: &'a Launch<'a>,
    world: &'a Rc<RefCell<dyn World>>,
    /// What a test is handed to: `test.outcome`.
    within: Symbol,
    /// What a test runs under: `test.runner`.
    target: Symbol,
    /// What a test that could not be run was refused with, as the
    /// command reports an error: an outcome holds only its message.
    refused: RefCell<BTreeMap<Symbol, String>>,
    /// Whether the run's end said a test failed.
    failed: Cell<bool>,
}

impl<'a> Answers<'a> {
    pub(super) fn new(
        launch: &'a Launch<'a>,
        world: &'a Rc<RefCell<dyn World>>,
        within: Symbol,
        target: Symbol,
    ) -> Self {
        Self {
            launch,
            world,
            within,
            target,
            refused: RefCell::default(),
            failed: Cell::default(),
        }
    }

    /// Whether the run's end said a test failed.
    pub(super) fn failed(&self) -> bool {
        self.failed.get()
    }

    /// The native code a test is run with: what `system` only computes.
    fn natives() -> Natives {
        crate::host::computing(Natives::new())
    }

    /// What running `test` came to, or an outcome that ended in what it
    /// was refused with.
    fn ran(&self, test: &Symbol) -> Value {
        let engine = self.launch.engine;
        let ran = engine.ran_within(test, &self.within, &self.target, &Self::natives());
        ran.unwrap_or_else(|error| self.refusal(test, &error))
    }

    /// What is kept of `test`, or null. One that cannot be run has
    /// nothing kept, so running it says why.
    fn kept(&self, test: &Symbol) -> Value {
        let engine = self.launch.engine;
        let kept = engine.kept_within(test, &self.within, &self.target, &Self::natives());
        kept.ok().flatten().unwrap_or(Value::Null)
    }

    /// An outcome that ended in `error`, which is kept to be reported as
    /// the command reports any.
    fn refusal(&self, test: &Symbol, error: &Error) -> Value {
        let said = crate::cli::report(self.launch.project, self.launch.engine, error);
        self.refused.borrow_mut().insert(test.clone(), said);
        Value::Record(BTreeMap::from([
            ("unmet".to_owned(), Value::List(Vec::new())),
            ("error".to_owned(), refused("failed", error.to_string())),
        ]))
    }

    /// Writes what an event (`test.Event`) says.
    fn report(&self, event: &Value) -> Result<(), String> {
        let Value::Record(event) = event else {
            return Err("an event is a record".to_owned());
        };
        let mut said = String::new();
        if let Some(test) = event.get("ended") {
            let function = definition(&[field(test, "function")], 0)?;
            let outcome = event.get("outcome").cloned().unwrap_or(Value::Null);
            let problems = match self.refused.borrow().get(&function) {
                Some(refused) => vec![refused.clone()],
                None => problems(&outcome, |error| self.trace(&function, error)),
            };
            let name = name(test);
            if problems.is_empty() {
                let _ = writeln!(said, "ok    {name}");
            } else {
                let _ = writeln!(said, "FAIL  {name}");
                for line in problems.iter().flat_map(|problem| problem.lines()) {
                    let _ = writeln!(said, "      {line}");
                }
            }
        } else if let Some(tally) = event.get("done") {
            let count = |name: &str| match field(tally, name) {
                Value::Int(count) => count,
                _ => 0,
            };
            let (passed, failed) = (count("passed"), count("failed"));
            let again = match count("kept") {
                0 => String::new(),
                kept => format!(" ({kept} not run again)"),
            };
            self.failed.set(failed > 0);
            let _ = writeln!(said, "{passed} passed{again}, {failed} failed");
        }
        self.world.borrow_mut().write(&said);
        Ok(())
    }

    /// The lines of an error's trace, less the frames of what ran it.
    fn trace(&self, test: &Symbol, error: &Value) -> Vec<String> {
        let engine = self.launch.engine;
        let Ok(root) = engine.program_within(test, &self.within, &self.target) else {
            return Vec::new();
        };
        let error = inner(error, &trace::reach(engine, &self.within));
        self.launch.frames(&root).trace(&error)
    }
}

impl Questions for Answers<'_> {
    fn answer(&self, question: &str, args: &[Value]) -> Result<Value, String> {
        match question {
            "test.run" => Ok(self.ran(&definition(args, 0)?)),
            "test.kept" => Ok(self.kept(&definition(args, 0)?)),
            "test.report" => {
                self.report(args.first().unwrap_or(&Value::Null))?;
                Ok(Value::Null)
            }
            _ => self.launch.engine.answer(question, args),
        }
    }
}

/// A field of a record, or null.
fn field(record: &Value, name: &str) -> Value {
    match record {
        Value::Record(fields) => fields.get(name).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// A test's name (`test.Test`), after those of its groups, outermost
/// first.
fn name(test: &Value) -> String {
    let mut names = Vec::new();
    let mut at = test.clone();
    while let Value::String(name) = &field(&at, "name") {
        names.push(name.clone());
        at = field(&at, "group");
    }
    names.reverse();
    names.join(".")
}
