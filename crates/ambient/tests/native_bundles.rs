//! Native code that reads its arguments where they lie, as a native
//! bundle's does. See "Native bundles" in `docs/spec/externs.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::backend::native::{Bundled, Calling, Natives, amb_bundle_call, amb_bundle_fail};
use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::{Fault, NULL, UNWIND, Word};
use support::{Outcome, run_natively};

/// The argument at `index`, which the call still owns.
unsafe fn arg(args: Word, index: u64) -> Word {
    unsafe { ambient_runtime::amb_word_at(args, index) }
}

/// Twice the integer it is handed.
unsafe extern "C" fn double(_: *mut Calling, args: Word) -> Word {
    unsafe { ambient_runtime::amb_int_new(ambient_runtime::amb_int_value(arg(args, 0)) * 2) }
}

/// How many bytes the text it is handed is, read from the text itself.
unsafe extern "C" fn size(_: *mut Calling, args: Word) -> Word {
    unsafe { ambient_runtime::amb_int_new(ambient_runtime::amb_length(arg(args, 0)).cast_signed()) }
}

/// What the function it is handed gives of the integer beside it.
unsafe extern "C" fn apply(call: *mut Calling, args: Word) -> Word {
    unsafe {
        let gave = amb_bundle_call(call, arg(args, 0), &arg(args, 1), 1);
        if gave == UNWIND { NULL } else { gave }
    }
}

unsafe extern "C" fn refuse(call: *mut Calling, _: Word) -> Word {
    let message = "no";
    unsafe { amb_bundle_fail(call, message.as_ptr(), message.len() as u64) };
    NULL
}

fn natives() -> Natives {
    let functions: [(&str, Bundled); 4] = [
        ("main.double", double),
        ("main.size", size),
        ("main.apply", apply),
        ("main.refuse", refuse),
    ];
    let offer = |natives: Natives, (path, function)| {
        // SAFETY: each keeps to what a function of a native bundle is
        // held to.
        unsafe { natives.bundled("app", path, function) }
    };
    functions.into_iter().fold(Natives::new(), offer)
}

/// An extern that takes `takes` and gives an integer.
fn takes<const N: usize>(name: &str, takes: [Type; N]) -> Definition {
    let params = takes.into_iter().enumerate();
    let function = params.fold(b::extern_function(name), |function, (index, takes)| {
        function.param(&format!("p{index}"), takes)
    });
    function.returns(ty::int()).into()
}

/// What `main` gives as `body`, beside every extern above.
fn outcome(body: Expr) -> Outcome {
    let externs = [
        takes("double", [ty::int()]),
        takes("size", [ty::string()]),
        takes("apply", [ty::function([ty::int()], ty::int()), ty::int()]),
        takes("refuse", []),
    ];
    let main = b::function("main").returns(ty::int()).body(body);
    run_natively(externs.into_iter().chain([main]), &natives())
}

#[test]
fn a_bundled_function_reads_an_argument_and_makes_what_it_gives() {
    let body = b::call(b::name("double"), [b::int(21)]);

    assert_eq!(outcome(body), Ok(Value::Int(42)));
}

#[test]
fn a_bundled_function_reads_a_value_where_it_lies() {
    let body = b::call(b::name("size"), [b::string("ambient")]);

    assert_eq!(outcome(body), Ok(Value::Int(7)));
}

#[test]
fn a_bundled_function_calls_a_function_with_what_it_captured() {
    let adds = b::lambda([("n", None)], b::add(b::name("n"), b::name("held")));
    let body = b::call(b::name("apply"), [adds, b::int(2)]);

    assert_eq!(
        outcome(b::let_("held", b::int(40), body)),
        Ok(Value::Int(42))
    );
}

#[test]
fn a_fault_in_what_a_bundled_function_calls_goes_on_through_it() {
    let faults = b::lambda([("n", None)], b::divide(b::name("n"), b::int(0)));
    let body = b::call(b::name("apply"), [faults, b::int(1)]);

    assert_eq!(outcome(body), Err(Fault::DivideByZero));
}

#[test]
fn a_bundled_function_that_says_it_failed_faults_its_task() {
    let body = b::call(b::name("refuse"), []);

    assert_eq!(outcome(body), Err(Fault::Native));
}
