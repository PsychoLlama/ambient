//! `ambient test`: the tests of a package, each run as a program of its
//! own, through `ambient.test.outcome`. See `docs/spec/testing.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use ambient_host::launch::describe;
use ambient_runtime::FAULT_PACKAGE;

use super::{Chosen, Launch, Session, trace};
use crate::backend::native::Natives;
use crate::engine::{Engine, Stage};
use crate::error::Error;
use crate::project::Project;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;
use crate::workspace::Sources;

/// What begins the name of a test.
const PREFIX: &str = "test_";

impl Session<'_> {
    /// Runs the tests of the packages `chosen` is, says how each
    /// went, and gives the code to exit with: zero if none failed.
    /// With `only`, the tests are those whose module path and name
    /// hold it.
    pub(super) fn test(&self, chosen: &Chosen, only: Option<&str>) -> Result<i32, Vec<String>> {
        let packages = self.each(chosen)?;
        let mut code = 0;
        for package in &packages {
            code = code.max(self.tested(package, only)?);
        }
        Ok(code)
    }

    /// Runs the tests of the one package at `path`.
    fn tested(&self, path: &Path, only: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let module = ModuleId::new(FAULT_PACKAGE, "test");
        let (target, within) = (module.value("runner"), module.value("outcome"));
        let (mut passed, mut failed) = (0, 0);
        let chosen = |test: &Symbol| only.is_none_or(|only| test.identity().contains(only));
        let tests = tests(&project, &engine)
            .map_err(|error| vec![super::report(&project, &engine, &error)])?;
        let tests: Vec<Symbol> = tests.into_iter().filter(chosen).collect();
        for test in &tests {
            // A test is a run kept as any other is, failed or not. Its
            // outcome names nothing, so it is named here, by what the
            // test is now.
            let natives = crate::host::computing(Natives::new());
            let problems = match engine.run_within(test, &within, &target, &natives) {
                Ok(outcome) => problems(&outcome, |error| {
                    let Ok(root) = engine.program_within(test, &within, &target) else {
                        return Vec::new();
                    };
                    let error = inner(error, &trace::reach(&engine, &within));
                    launch.frames(&root).trace(&error)
                }),
                Err(error) => vec![super::report(&project, &engine, &error)],
            };
            let mut world = self.world.borrow_mut();
            if problems.is_empty() {
                passed += 1;
                world.write(&format!("ok    {test}\n"));
            } else {
                failed += 1;
                world.write(&format!("FAIL  {test}\n"));
                for line in problems.iter().flat_map(|problem| problem.lines()) {
                    world.write(&format!("      {line}\n"));
                }
            }
        }
        let reused = engine.reused(Stage::Run);
        let kept = tests
            .iter()
            .filter(|test| reused.contains(&test.to_string()));
        let again = match kept.count() {
            0 => String::new(),
            kept => format!(" ({kept} not run again)"),
        };
        let summary = format!("{passed} passed{again}, {failed} failed\n");
        self.world.borrow_mut().write(&summary);
        Ok(i32::from(failed > 0))
    }
}

/// Whether a definition of this name and kind is a test.
fn test<N>(name: &str, kind: &DefinitionKind<N>) -> bool {
    let takes_nothing =
        matches!(kind, DefinitionKind::Function(function) if function.params.is_empty());
    takes_nothing && name.starts_with(PREFIX)
}

/// The tests of the package a project was opened at, in the order of
/// their modules and of their definitions there: each function that
/// takes nothing and is named as a test. Those a generated module was
/// given follow what the module was written with.
fn tests(project: &Project, engine: &Engine) -> Result<Vec<Symbol>, Error> {
    let root = ModuleId::new(&project.root, "").package;
    let mut tests = Vec::new();
    for read in project
        .modules
        .iter()
        .filter(|read| read.module.package == root)
    {
        let Some(module) = project.workspace.module(&read.module) else {
            continue;
        };
        let written = module.definitions().iter();
        let written = written.filter(|definition| test(&definition.name, &definition.kind));
        tests.extend(written.map(|definition| read.module.value(&definition.name)));
        for made in engine.generated_in(&read.module)? {
            let resolved = engine.resolve(&made)?;
            if test(&made.name, &resolved.definition.kind) {
                tests.push(made);
            }
        }
    }
    Ok(tests)
}

/// An error a test ended with, less the frames of what ran it: those at
/// the outer end of its trace whose definitions are in `harness`. A
/// frame of the test's own stops the cut, so what the test itself
/// calls of the harness is kept.
fn inner(error: &Value, harness: &BTreeSet<String>) -> Value {
    let Value::Record(fields) = error else {
        return error.clone();
    };
    let mut fields = fields.clone();
    if let Some(Value::List(frames)) = fields.get_mut("trace") {
        let ran = |frame: &Value| match frame {
            Value::Record(frame) => matches!(
                frame.get("definition"),
                Some(Value::String(hash)) if harness.contains(hash)
            ),
            _ => false,
        };
        while frames.last().is_some_and(ran) {
            frames.pop();
        }
    }
    Value::Record(fields)
}

/// What is said of a test's outcome (`ambient.test.Outcome`): each
/// check that did not hold, then the error that ended it, if one did,
/// with a line for each frame of its trace, which `trace` gives.
fn problems(outcome: &Value, trace: impl FnOnce(&Value) -> Vec<String>) -> Vec<String> {
    let Value::Record(outcome) = outcome else {
        return vec!["a test came to what is no outcome".to_owned()];
    };
    let unmet = match outcome.get("unmet") {
        Some(Value::List(unmet)) => unmet.as_slice(),
        _ => &[],
    };
    let mut problems: Vec<String> = unmet
        .iter()
        .map(|failure| match failure {
            Value::Record(failure) => told(failure),
            _ => "does not hold: a check".to_owned(),
        })
        .collect();
    if let Some(error) = outcome.get("error").filter(|error| **error != Value::Null) {
        let mut lines = vec![describe(error)];
        lines.extend(trace(error));
        problems.push(lines.join("\n"));
    }
    problems
}

/// What is said of a failure: what did not hold, and under it what
/// was found and what was expected, if it holds them.
fn told(failure: &BTreeMap<String, Value>) -> String {
    let what = match failure.get("what") {
        Some(Value::String(what)) => what.as_str(),
        _ => "a check",
    };
    let mut said = format!("does not hold: {what}");
    for part in ["found", "expected"] {
        if let Some(value) = failure.get(part) {
            let _ = write!(said, "\n  {part}: {}", written(value));
        }
    }
    said
}

/// A value as a person reads it, written as source is where source
/// can write it.
fn written(value: &Value) -> String {
    let all = |values: &[Value]| values.iter().map(written).collect::<Vec<_>>().join(", ");
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => format!("{value:?}"),
        Value::String(text) => format!("{text:?}"),
        Value::Bytes(bytes) => {
            let bytes: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("0x[{}]", bytes.join(" "))
        }
        Value::Tag { tag, .. } => tag.clone(),
        Value::Tuple(members) => format!("({})", all(members)),
        Value::List(members) => format!("[{}]", all(members)),
        Value::Record(fields) => {
            let fields = fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", written(value)));
            format!("{{ {} }}", fields.collect::<Vec<_>>().join(", "))
        }
        Value::Handle { brand, .. } => format!("a handle of `{brand}`"),
        Value::Function(_) | Value::Method(_) => "a function".to_owned(),
        Value::Iterator(..) => "an iterator".to_owned(),
        Value::Cell | Value::Token | Value::Types => "what only its program holds".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{BTreeSet, Value, inner, problems, told, written};

    fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
        Value::Record(BTreeMap::from(
            fields.map(|(name, value)| (name.to_owned(), value)),
        ))
    }

    #[test]
    fn a_value_is_written_as_source_writes_it() {
        let record = BTreeMap::from([("on".to_owned(), Value::Bool(true))]);
        let value = Value::List(vec![
            Value::Tuple(vec![Value::Int(1), Value::String("a\"b".to_owned())]),
            Value::Record(record),
            Value::Bytes(vec![0x89, 0x50]),
            Value::Null,
        ]);

        assert_eq!(
            written(&value),
            r#"[(1, "a\"b"), { on: true }, 0x[89 50], null]"#
        );
    }

    #[test]
    fn a_failure_says_what_was_found_and_expected_if_it_holds_them() {
        let what = ("what".to_owned(), Value::String("both lights".to_owned()));
        let plain = BTreeMap::from([what.clone()]);
        let found = ("found".to_owned(), Value::Int(1));
        let differs = BTreeMap::from([what, found, ("expected".to_owned(), Value::Int(2))]);

        assert_eq!(told(&plain), "does not hold: both lights");
        assert_eq!(
            told(&differs),
            "does not hold: both lights\n  found: 1\n  expected: 2"
        );
    }

    #[test]
    fn an_outcome_says_each_unmet_check_then_the_error_that_ended_it() {
        let unmet = record([("what", Value::String("it holds".to_owned()))]);
        let error = record([
            ("message", Value::String("not yet".to_owned())),
            ("trace", Value::List(Vec::new())),
        ]);
        let outcome = record([("unmet", Value::List(vec![unmet])), ("error", error)]);
        let passed = record([("unmet", Value::List(Vec::new())), ("error", Value::Null)]);

        assert_eq!(
            problems(&outcome, |_| vec!["  in a test".to_owned()]),
            ["does not hold: it holds", "fault: not yet\n  in a test"]
        );
        assert!(problems(&passed, |_| panic!("nothing to name")).is_empty());
    }

    #[test]
    fn what_ran_a_test_is_cut_from_the_outer_end_of_its_trace() {
        let frame = |hash: &str| record([("definition", Value::String(hash.to_owned()))]);
        let trace = |hashes: &[&str]| {
            let frames = hashes.iter().map(|hash| frame(hash)).collect();
            record([("trace", Value::List(frames))])
        };
        let harness = BTreeSet::from(["catch".to_owned(), "outcome".to_owned()]);

        assert_eq!(
            inner(&trace(&["catch", "test", "catch", "outcome"]), &harness),
            trace(&["catch", "test"])
        );
    }
}
