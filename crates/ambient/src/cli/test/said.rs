//! What is said of a test's outcome, as a person reads it: pure
//! functions of the values the program that runs tests reports.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use ambient_host::launch::describe;

use crate::value::Value;

/// An error a test ended with, less the frames of what ran it: those at
/// the outer end of its trace whose definitions are in `harness`. A
/// frame of the test's own stops the cut, so what the test itself
/// calls of the harness is kept.
pub(super) fn inner(error: &Value, harness: &BTreeSet<String>) -> Value {
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
pub(super) fn problems(outcome: &Value, trace: impl FnOnce(&Value) -> Vec<String>) -> Vec<String> {
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
