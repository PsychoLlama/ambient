//! Finding tests: the functions a package's manifest lists, or the
//! standard library's, each a tool handed a selection of the package.
//! See "Finding" in `docs/spec/testing.md`.

use ambient_runtime::FAULT_PACKAGE;

use super::Engine;
use super::asked::definition;
use super::built::built;
use crate::error::Error;
use crate::symbol::{ModuleId, PackageId, Symbol};
use crate::value::Value;

/// A test, as a way to find tests gave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The names of the groups it is in, outermost first, then its own.
    pub name: Vec<String>,
    /// The function that is the test.
    pub function: Symbol,
    /// The test as it was given: a `test.Test`.
    pub test: Value,
}

/// The standard library's `test` module.
fn testing() -> ModuleId {
    ModuleId::new(FAULT_PACKAGE, "test")
}

/// The tests a way to find them gave. What a checked one gives is
/// `test.Tests`, whatever it did, so what is not a test is passed by.
fn tests_of(given: &Value) -> Vec<Found> {
    let test = |given: &Value| {
        let Value::Record(fields) = given else {
            return None;
        };
        let function = definition(&[fields.get("function")?.clone()], 0).ok()?;
        // The test, then each group out from it.
        let mut name = Vec::new();
        let mut at = Some(given);
        while let Some(Value::Record(fields)) = at {
            let Some(Value::String(named)) = fields.get("name") else {
                return None;
            };
            name.push(named.clone());
            at = fields.get("group");
        }
        name.reverse();
        Some(Found {
            name,
            function,
            test: given.clone(),
        })
    };
    match given {
        Value::List(given) => given.iter().filter_map(test).collect(),
        _ => Vec::new(),
    }
}

impl Engine<'_> {
    /// What runs the tests of `package`: what its manifest names, or
    /// `system.test.tests`.
    #[must_use]
    pub fn runner(&self, package: &PackageId) -> Symbol {
        let runner = self.sources.runner(package);
        let standard = || ModuleId::new(crate::system::PACKAGE, "test").value("tests");
        runner.unwrap_or_else(standard)
    }

    /// The tests of `package`: what each way to find them its manifest
    /// lists gives, in order, or what `test.annotated` does if it lists
    /// none. A function two of them give is a test once, where it was
    /// first given.
    ///
    /// Each is handed every module of the package and no text a name
    /// must hold: what runs the tests chooses among them, so what was
    /// found is kept whatever is chosen.
    ///
    /// # Errors
    ///
    /// Fails if what is listed is not a way to find tests, or as
    /// [`Self::tool`] does.
    pub fn found(&self, package: &PackageId) -> Result<Vec<Found>, Error> {
        let mut finders = self.sources.finders(package);
        if finders.is_empty() {
            finders.push(testing().value("annotated"));
        }
        let tests = testing().ty("Tests");
        let selection = self.selection(package, "");
        let mut found: Vec<Found> = Vec::new();
        for finder in finders {
            if !self.typed(&finder, &built("Selection"), &tests)? {
                return Err(Error::NotFinder(finder));
            }
            let given = self.tool(&finder, std::slice::from_ref(&selection))?;
            for test in tests_of(&given) {
                if !found.iter().any(|seen| seen.function == test.function) {
                    found.push(test);
                }
            }
        }
        Ok(found)
    }
}
