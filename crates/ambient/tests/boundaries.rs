//! Boundaries, run on every backend. See `docs/spec/values.md` and
//! `docs/spec/errors.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::builder::{self as b, pat, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::{FAULT_PACKAGE, Fault};
use support::{kept, run, value};

fn outcome(
    ret: Type,
    body: Expr,
    extra: impl IntoIterator<Item = Definition>,
) -> Result<Value, Fault> {
    let main = b::function("main").returns(ret).body(body);
    run(extra.into_iter().chain([main]), &[])
}

fn evaluates(body: Expr) -> Value {
    outcome(ty::unknown(), body, []).unwrap()
}

fn overflows() -> Expr {
    b::add(b::int(i64::MAX), b::int(1))
}

/// The kind of the error `value` is.
fn kind(value: &Value) -> Value {
    match value {
        Value::Record(fields) => fields["kind"].clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn fault(tag: &str) -> Value {
    Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: tag.to_owned(),
    }
}

#[test]
fn a_boundary_is_what_its_body_gives() {
    assert_eq!(evaluates(b::boundary(b::int(1))), Value::Int(1));
}

#[test]
fn a_boundary_gives_the_fault_that_ended_its_body_as_an_error() {
    let Value::Record(ref error) = evaluates(b::boundary(overflows())) else {
        panic!("expected an error");
    };

    assert_eq!(error["kind"], fault("overflow"));
    assert!(matches!(error["message"], Value::String(_)));
    assert_eq!(error["cause"], Value::Null);
    assert!(matches!(error["trace"], Value::List(_)));
}

#[test]
fn the_rest_of_the_program_goes_on_after_a_contained_fault() {
    let arms = [(pat::is(ty::int()), b::int(1)), (pat::any(), b::int(2))];
    let body = b::add(b::match_(b::boundary(overflows()), arms), b::int(40));

    assert_eq!(outcome(ty::int(), body, []), Ok(Value::Int(42)));
}

#[test]
fn a_boundary_has_the_type_of_its_body_or_an_error() {
    let error = ty::record([
        ("kind", ty::tag()),
        ("message", ty::string()),
        ("cause", ty::null()),
    ]);
    let body = || b::boundary(b::int(1));

    assert!(outcome(ty::union([ty::int(), error]), body(), []).is_ok());
}

#[test]
fn an_abort_passes_a_boundary() {
    let fail = b::ability("fail").returns(ty::never());
    let body = b::provide(
        [("fail", b::lambda([], b::int(-1)))],
        b::boundary(b::perform("fail", [])),
    );

    assert_eq!(
        outcome(ty::unknown(), body, [fail.into()]),
        Ok(Value::Int(-1))
    );
}

#[test]
fn a_boundary_contains_a_fault_in_a_task_it_started() {
    let tasks = b::all([b::int(1), overflows()]);

    assert_eq!(kind(&evaluates(b::boundary(tasks))), fault("overflow"));
}

#[test]
fn cleanups_run_before_a_boundary_has_its_fault() {
    let c = || b::name("c");
    let noted = support::cleanup(b::replace(c(), b::int(0), b::int(1)), overflows());
    let body = b::let_(
        "c",
        b::cell(ty::int(), b::int(0)),
        b::tuple([b::boundary(noted), b::read(c())]),
    );
    let Value::Tuple(ref members) = evaluates(body) else {
        panic!("expected a pair");
    };

    assert_eq!(kind(&members[0]), fault("overflow"));
    assert_eq!(members[1], Value::Int(1));
}

#[test]
fn a_program_that_stalls_inside_a_boundary_still_ends() {
    let c = || b::name("c");
    let waits = b::wait(c(), b::int(0));
    let body = b::let_("c", b::cell(ty::int(), b::int(0)), b::boundary(waits));

    assert_eq!(outcome(ty::unknown(), body, []), Err(Fault::Cancelled));
}

// Traces

/// The trace of the error a boundary around `body` gives: for each frame,
/// which of `extra` or `main` its definition is, by name, and its
/// position there. A lambda has no name, and is `"?"`.
fn trace<const N: usize>(body: Expr, extra: [Definition; N]) -> Vec<(String, i64)> {
    let main = b::function("main")
        .returns(ty::unknown())
        .body(b::boundary(body));
    let names: Vec<String> = extra.iter().map(|d| d.name.clone()).collect();
    let definitions: Vec<Definition> = extra.into_iter().chain([main]).collect();

    // The same program, to learn the hash of each definition in it.
    let workspace = support::program(definitions.iter().cloned());
    let store = ambient::store::MemoryStore::new();
    let engine = ambient::engine::Engine::new(&workspace, &store);
    let owner = |hash: &str| {
        let is = |name: &&str| engine.hash(&value("main", name)).unwrap().to_string() == hash;
        let names = names.iter().map(String::as_str).chain(["main"]);
        names.clone().find(is).unwrap_or("?").to_owned()
    };

    let Value::Record(ref error) = run(definitions, &[]).unwrap() else {
        panic!("expected an error");
    };
    let Value::List(frames) = &error["trace"] else {
        panic!("expected a list of frames");
    };
    let frame = |frame: &Value| match frame {
        Value::Record(fields) => match (&fields["definition"], &fields["position"]) {
            (Value::String(hash), Value::Int(position)) => (owner(hash), *position),
            other => panic!("expected a hash and a position, got {other:?}"),
        },
        other => panic!("expected a frame, got {other:?}"),
    };
    frames.iter().map(frame).collect()
}

fn at(name: &str, position: i64) -> (String, i64) {
    (name.to_owned(), position)
}

/// `fn name(): int`
fn giving(name: &str, body: Expr) -> Definition {
    b::function(name).returns(ty::int()).body(body)
}

#[test]
fn a_trace_says_where_the_fault_was() {
    // The boundary is the first node of `main`, and the sum the second.
    assert_eq!(trace(overflows(), []), [at("main", 1)]);
}

#[test]
fn a_trace_has_a_frame_for_each_call_the_fault_left() {
    let inner = giving("inner", overflows());
    let outer = giving("outer", kept(b::call(b::name("inner"), [])));

    assert_eq!(
        trace(b::call(b::name("outer"), []), [inner, outer]),
        [at("inner", 0), at("outer", 1), at("main", 1)]
    );
}

#[test]
fn a_trace_goes_on_from_a_task_to_what_started_it() {
    let frames = trace(b::all([overflows(), b::null()]), []);

    // The task's body is a definition of its own, then the `all`.
    assert_eq!(frames, [at("?", 0), at("main", 1)]);
}

#[test]
fn a_trace_is_kept_while_a_cleanup_runs() {
    let body = support::cleanup(b::lambda([], b::int(0)), overflows());

    // The fault is in the body, after everything the using held.
    assert_eq!(trace(body, []), [at("main", 10)]);
}

#[test]
fn a_trace_keeps_only_the_frames_nearest_the_fault() {
    let deeper = kept(b::call(
        b::name("down"),
        [b::subtract(b::name("n"), b::int(1))],
    ));
    let down = b::function("down")
        .param("n", ty::int())
        .returns(ty::int())
        .body(b::match_(
            b::name("n"),
            [(pat::int(0), overflows()), (pat::any(), deeper)],
        ));
    let frames = trace(b::call(b::name("down"), [b::int(100)]), [down]);

    assert_eq!(frames.len(), 64);
    assert!(frames.iter().all(|(name, _)| name == "down"));
}
