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
use support::{run_entry, value, verify_all, workspace};

/// `fn main(): unknown`
fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::unknown()).body(body)
}

/// `fn here(): unknown`, which is the trace of where it is.
fn here() -> Definition {
    b::function("here").returns(ty::unknown()).body(b::trace())
}

/// Runs `main` among `definitions`, and names the definition of each
/// frame of the trace it gives: a definition written here by its name,
/// and any other, a lambda, as `"?"`.
fn traced(definitions: impl IntoIterator<Item = Definition>) -> Vec<String> {
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
    match run_entry(&engine, &entry, &[], Natives::new).unwrap() {
        Value::List(ref frames) => frames.iter().map(|frame| match frame {
            Value::Record(fields) => name(&fields["definition"]),
            other => panic!("expected a frame, got {other:?}"),
        }),
        ref other => panic!("expected a trace, got {other:?}"),
    }
    .collect()
}

#[test]
fn a_trace_begins_where_it_is_captured() {
    assert_eq!(traced([main(b::trace())]), ["main"]);
}

#[test]
fn a_trace_has_a_frame_for_each_call_it_is_inside() {
    let middle = b::function("middle")
        .returns(ty::unknown())
        .body(b::call(b::name("here"), []));
    let outer = main(b::call(b::name("middle"), []));
    assert_eq!(traced([here(), middle, outer]), ["here", "middle", "main"]);
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
    let branch = b::call(b::name("here"), []);
    let first = b::match_(
        b::all([branch]),
        [(b::pat::tuple([b::pat::bind("trace")]), b::name("trace"))],
    );
    assert_eq!(traced([here(), main(first)]), ["here", "?", "main"]);
}

#[test]
fn a_trace_keeps_the_frames_nearest_where_it_was_captured() {
    // `down(n)` calls itself `n` times and then captures.
    let deeper = b::call(b::name("down"), [b::subtract(b::name("n"), b::int(1))]);
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
