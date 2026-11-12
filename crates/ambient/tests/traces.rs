//! Capturing a trace where a program asks for one. See `spec/errors.md`.
//!
//! The trace of a fault is tested in `boundaries.rs`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::Natives;
use ambient::builder::{self as b, ModuleBuilder, ty};
use ambient::engine::Engine;
use ambient::store::MemoryStore;
use ambient::surface::{Definition, Expr};
use ambient::value::Value;
use support::{Outcome, kept, run_for_error, value, verify_all, workspace};

/// `fn main(): unknown`
fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::unknown()).body(body)
}

/// `fn here(): unknown`, which is the trace of where it is.
fn here() -> Definition {
    b::function("here").returns(ty::unknown()).body(b::trace())
}

/// Runs `main` among `definitions`, and gives `trace_of` the outcome
/// and the error a fault that ended it was. Each frame of the trace it
/// picks is named by its definition: one written here by its name, and
/// any other, a lambda, as `"?"`.
fn frames(
    definitions: impl IntoIterator<Item = Definition>,
    trace_of: impl Fn(Outcome, Option<Value>) -> Value,
) -> Vec<String> {
    let definitions: Vec<Definition> = definitions.into_iter().collect();
    let names: Vec<String> = definitions.iter().map(|d| d.name.clone()).collect();
    let module = definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    let workspace = workspace([("app", "main", module)]);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let entry = value("main", "main");
    verify_all(&engine, &entry);
    let name = |hash: &Value| {
        let is_frame = |name: &&String| {
            let hash_of = engine.hash(&value("main", name)).unwrap().to_string();
            Value::String(hash_of) == *hash
        };
        names
            .iter()
            .find(is_frame)
            .map_or("?", |name| name)
            .to_owned()
    };
    let (outcome, error) = run_for_error(&engine, &entry, &[], Natives::new);
    match trace_of(outcome, error) {
        Value::List(ref frames) => frames.iter().map(|frame| match frame {
            Value::Record(fields) => name(&fields["definition"]),
            other => panic!("expected a frame, got {other:?}"),
        }),
        ref other => panic!("expected a trace, got {other:?}"),
    }
    .collect()
}

/// The frames of the trace `main` gives.
fn traced(definitions: impl IntoIterator<Item = Definition>) -> Vec<String> {
    frames(definitions, |outcome, _| outcome.unwrap())
}

#[test]
fn a_trace_begins_where_it_is_captured() {
    assert_eq!(traced([main(b::trace())]), ["main"]);
}

#[test]
fn a_trace_has_a_frame_for_each_call_it_is_inside() {
    let middle = b::function("middle")
        .returns(ty::unknown())
        .body(kept(b::call(b::name("here"), [])));
    let outer = main(kept(b::call(b::name("middle"), [])));
    assert_eq!(traced([here(), middle, outer]), ["here", "middle", "main"]);
}

#[test]
fn a_function_that_left_for_its_last_call_is_in_no_trace() {
    let middle = b::function("middle")
        .returns(ty::unknown())
        .body(b::call(b::name("here"), []));
    let outer = main(kept(b::call(b::name("middle"), [])));
    assert_eq!(traced([here(), middle, outer]), ["here", "main"]);
}

#[test]
fn a_program_that_left_for_its_last_call_is_in_no_trace() {
    // Nothing compiled is left to resume the capture: the host does.
    let outer = main(b::call(b::name("here"), []));
    assert_eq!(traced([here(), outer]), ["here"]);
}

#[test]
fn what_was_computed_before_a_capture_is_still_there_after() {
    // The capture suspends `main` and resumes it, and `x` comes back.
    let both = b::tuple([b::name("x"), b::name("trace")]);
    let body = b::let_("x", b::int(42), b::let_("trace", b::trace(), both));
    let module = b::module().define(main(body));
    let outcome = support::run_in(&workspace([("app", "main", module)])).unwrap();
    let Value::Tuple(ref members) = outcome else {
        panic!("expected a tuple, got {outcome:?}");
    };
    assert_eq!(members[0], Value::Int(42));
    assert!(matches!(&members[1], Value::List(frames) if frames.len() == 1));
}

#[test]
fn the_frames_of_a_task_are_followed_by_those_of_what_started_it() {
    let branch = kept(b::call(b::name("here"), []));
    let first = b::match_(
        b::all([branch]),
        [(b::pat::tuple([b::pat::bind("trace")]), b::name("trace"))],
    );
    assert_eq!(traced([here(), main(first)]), ["here", "?", "main"]);
}

#[test]
fn a_trace_keeps_the_frames_nearest_where_it_was_captured() {
    // `down(n)` calls itself `n` times and then captures.
    let deeper = kept(b::call(
        b::name("down"),
        [b::subtract(b::name("n"), b::int(1))],
    ));
    let down = b::function("down")
        .param("n", ty::int())
        .returns(ty::unknown())
        .body(b::match_(
            b::name("n"),
            [(b::pat::int(0), b::trace()), (b::pat::any(), deeper)],
        ));
    let frames = traced([down, main(b::call(b::name("down"), [b::int(100)]))]);
    assert_eq!(frames.len(), ambient_runtime::TRACE_LIMIT);
    assert!(frames.iter().all(|frame| frame == "down"));
}

#[test]
fn a_fault_that_ends_a_program_reaches_its_host_with_its_trace() {
    let overflows = b::function("overflows")
        .returns(ty::int())
        .body(b::add(b::int(i64::MAX), b::int(1)));
    let outer = main(kept(b::call(b::name("overflows"), [])));
    let trace = |outcome: Outcome, error: Option<Value>| {
        assert_eq!(outcome, Err(ambient_runtime::Fault::Overflow));
        match error {
            Some(Value::Record(ref fields)) => fields["trace"].clone(),
            other => panic!("expected an error, got {other:?}"),
        }
    };
    assert_eq!(frames([overflows, outer], trace), ["overflows", "main"]);
}
