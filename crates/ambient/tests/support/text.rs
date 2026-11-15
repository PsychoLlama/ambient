//! Programs written as text.

use ambient::symbol::ModuleId;
use ambient::syntax::parse;
use ambient::value::Value;
use ambient::workspace::Workspace;

use super::{Outcome, run_in};

/// A workspace whose package `app` has the standard library, the
/// `system` package, and one
/// module, `main`, written as `source`.
pub fn workspace(source: &str) -> Workspace {
    let module = parse(source).unwrap_or_else(|error| panic!("{error}"));
    let mut workspace = Workspace::new();
    ambient::library::install(&mut workspace).unwrap();
    ambient::system::install(&mut workspace).unwrap();
    workspace.depend("app", "ambient", "ambient").unwrap();
    workspace.depend("app", "system", "system").unwrap();
    workspace.set(ModuleId::new("app", "main"), module).unwrap();
    workspace
}

/// Runs `main` of a program written as one module, on every backend.
pub fn run(source: &str) -> Outcome {
    run_in(&workspace(source))
}

/// What `main` gives, which is an integer.
pub fn int(source: &str) -> i64 {
    match run(source) {
        Ok(Value::Int(value)) => value,
        other => panic!("expected an integer, got {other:?}"),
    }
}

/// What `main` gives, which is a string.
pub fn text(source: &str) -> String {
    match &run(source) {
        Ok(Value::String(value)) => value.clone(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// What `fn main(): string => <expr>;` gives.
pub fn evaluates(expr: &str) -> String {
    text(&format!("fn main(): string => {expr};"))
}
