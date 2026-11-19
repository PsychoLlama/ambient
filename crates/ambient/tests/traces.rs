//! The trace of an abort, which its handler takes. See `docs/spec/errors.md`.
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

/// `ability stop(): never`
fn stop() -> Definition {
    b::ability("stop").returns(ty::never()).into()
}

/// `fn main(): unknown`, which is `body` under a handler of `stop` that
/// gives the trace of the abort it answers.
fn main(body: Expr) -> Definition {
    let traces = b::lambda([("trace", Some(ty::unknown()))], b::name("trace"));
    let caught = b::provide([("stop", traces)], body);
    b::function("main").returns(ty::unknown()).body(caught)
}

/// `fn name(): unknown with stop`
fn stopping(name: &str, body: Expr) -> Definition {
    b::function(name)
        .returns(ty::unknown())
        .performs(["stop"])
        .body(body)
}

/// `fn here(): unknown with stop`, which stops.
fn here() -> Definition {
    stopping("here", b::perform("stop", []))
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
            // An ability is among them, and is no function.
            let hash_of = engine.hash(&value("main", name)).ok();
            hash_of.is_some_and(|found| Value::String(found.to_string()) == *hash)
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
    let definitions = [stop()].into_iter().chain(definitions);
    frames(definitions, |outcome, _| outcome.unwrap())
}

#[test]
fn a_trace_begins_where_the_abort_was_performed() {
    assert_eq!(traced([main(b::perform("stop", []))]), ["main"]);
}

#[test]
fn a_trace_has_a_frame_for_each_call_the_abort_left() {
    let middle = stopping("middle", kept(b::call(b::name("here"), [])));
    let outer = main(kept(b::call(b::name("middle"), [])));
    assert_eq!(traced([here(), middle, outer]), ["here", "middle", "main"]);
}

#[test]
fn a_function_that_left_for_its_last_call_is_in_no_trace() {
    let middle = stopping("middle", b::call(b::name("here"), []));
    let outer = main(kept(b::call(b::name("middle"), [])));
    assert_eq!(traced([here(), middle, outer]), ["here", "main"]);
}

#[test]
fn the_frames_of_a_task_are_followed_by_those_of_what_started_it() {
    let branch = kept(b::call(b::name("here"), []));
    let both = b::all([branch, b::null()]);
    assert_eq!(traced([here(), main(both)]), ["here", "?", "main"]);
}

#[test]
fn a_trace_keeps_the_frames_nearest_where_the_abort_began() {
    // `down(n)` calls itself `n` times and then stops.
    let deeper = kept(b::call(
        b::name("down"),
        [b::subtract(b::name("n"), b::int(1))],
    ));
    let down = b::function("down")
        .param("n", ty::int())
        .returns(ty::unknown())
        .performs(["stop"])
        .body(b::match_(
            b::name("n"),
            [
                (b::pat::int(0), b::perform("stop", [])),
                (b::pat::any(), deeper),
            ],
        ));
    let frames = traced([down, main(b::call(b::name("down"), [b::int(100)]))]);
    assert_eq!(frames.len(), ambient_runtime::TRACE_LIMIT);
    assert!(frames.iter().all(|frame| frame == "down"));
}

#[test]
fn a_handler_that_takes_no_trace_gathers_none() {
    // The handler answers with what the abort was performed with.
    let answers = b::lambda([], b::int(7));
    let body = b::provide([("stop", answers)], b::call(b::name("here"), []));
    let main = b::function("main").returns(ty::unknown()).body(body);
    let module = [stop(), here(), main]
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    let outcome = support::run_in(&workspace([("app", "main", module)]));
    assert_eq!(outcome, Ok(Value::Int(7)));
}

#[test]
fn a_fault_that_ends_a_program_reaches_its_host_with_its_trace() {
    let overflows = b::function("overflows")
        .returns(ty::int())
        .body(b::add(b::int(i64::MAX), b::int(1)));
    let outer = b::function("main")
        .returns(ty::unknown())
        .body(kept(b::call(b::name("overflows"), [])));
    let trace = |outcome: Outcome, error: Option<Value>| {
        assert_eq!(outcome, Err(ambient_runtime::Fault::Overflow));
        match error {
            Some(Value::Record(ref fields)) => fields["trace"].clone(),
            other => panic!("expected an error, got {other:?}"),
        }
    };
    assert_eq!(frames([overflows, outer], trace), ["overflows", "main"]);
}

/// How many frames the error `main` catches has, in a program that
/// begins with `breaks`, which throws.
fn caught_frames(main: &str) -> i64 {
    support::text::int(&format!(
        r#"
        use ambient.{{catch, fail, throw, Error}};
        tag broken;

        fn breaks(): int with throw {{ fail(broken, "no"); return 0; }}
        fn frames(caught: int | Error): int {{
            match caught {{
                let error: Error => {{ return error.trace.length(); }}
                let other: int => {{ return other; }}
            }}
        }}
        {main}
        "#
    ))
}

#[test]
fn an_error_is_caught_with_the_trace_of_its_throw() {
    // Where it was thrown, the call of that, and the call `catch` made.
    let once = "fn main(): int => frames(catch(fn() => breaks()));";
    assert_eq!(caught_frames(once), 3);
}

#[test]
fn an_error_thrown_again_says_both_ways_it_came() {
    let twice = r"
        fn again(): int with throw {
            match catch(fn() => breaks()) {
                let error: Error => { throw!(error); return 0; }
                let other: int => { return other; }
            }
        }
        fn main(): int => frames(catch(fn() => again()));
    ";
    // Those three, then where it was thrown again and the call `catch`
    // made there.
    assert_eq!(caught_frames(twice), 5);
}
