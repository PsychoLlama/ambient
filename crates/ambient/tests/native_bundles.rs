//! Native code that reads its arguments where they lie, as a native
//! bundle's does. See "Native bundles" in `docs/spec/externs.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::sync::atomic::{AtomicU64, Ordering};

use ambient::backend::native::{Bundled, Calling, Natives, amb_bundle_call};
use ambient::builder::{self as b, ty};
use ambient::surface::{Definition, Expr, Type};
use ambient::value::Value;
use ambient_runtime::{Fault, NULL, UNWIND, Word};
use support::{Outcome, run_natively};

/// The argument at `index`, which the call still owns.
unsafe fn arg(args: Word, index: u64) -> Word {
    unsafe { ambient_runtime::amb_word_at(args, index) }
}

// What a bundle written in Rust writes: each function is named for
// its extern, and reads and makes values through the runtime.
ambient_native::natives! {
    package "app";

    /// Twice the integer it is handed.
    fn main_double(_call, args) {
        unsafe { ambient_native::amb_int_new(ambient_native::amb_int_value(arg(args, 0)) * 2) }
    }

    /// How many bytes the text it is handed is, read from the text
    /// itself.
    fn main_size(_call, args) {
        let size = unsafe { ambient_native::text(arg(args, 0)) }.len();
        unsafe { ambient_native::amb_int_new(i64::try_from(size).unwrap()) }
    }

    fn main_refuse(call, _args) {
        unsafe { ambient_native::fail(call, "no") }
    }

    /// A file the bundle knows as `MINTED`, of which it asks to be
    /// told when the program holds it no longer.
    fn main_mint(call, _args) {
        let brand = b"app\0main.File";
        unsafe {
            let brand = ambient_native::amb_tag(brand.as_ptr(), brand.len() as u64);
            ambient_native::amb_bundle_dropping(call, brand, closed);
            ambient_native::amb_handle_new(brand, MINTED)
        }
    }
}

/// The number of the one file the bundle mints.
const MINTED: u64 = 7;

/// The number of the file the bundle was last told was dropped.
static CLOSED: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn closed(number: u64) {
    CLOSED.store(number, Ordering::Relaxed);
}

/// What the function it is handed gives of the integer beside it.
unsafe extern "C" fn apply(call: *mut Calling, args: Word) -> Word {
    unsafe {
        let gave = amb_bundle_call(call, arg(args, 0), &arg(args, 1), 1);
        if gave == UNWIND { NULL } else { gave }
    }
}

fn natives() -> Natives {
    let functions: [(&str, Bundled); 5] = [
        ("main.mint", main_mint),
        ("main.double", main_double),
        ("main.size", main_size),
        ("main.apply", apply),
        ("main.refuse", main_refuse),
    ];
    let offer = |natives: Natives, (path, function)| {
        // SAFETY: each keeps to what a function of a native bundle is
        // held to.
        unsafe { natives.bundled("app", path, function) }
    };
    functions.into_iter().fold(Natives::new(), offer)
}

/// An ability that takes `takes` and gives `gives`, and its native
/// handler, which is its default under `world`.
fn native<const N: usize>(name: &str, takes: [Type; N], gives: Type) -> [Definition; 2] {
    let params = takes.into_iter().enumerate();
    let ability = params.fold(b::ability(name), |ability, (index, takes)| {
        ability.param(&format!("p{index}"), takes)
    });
    let handler = b::native_handler(name, name).under("world");
    [ability.returns(gives).into(), handler.into()]
}

/// What `main` gives as `body`, beside every native handler above.
fn outcome(body: Expr) -> Outcome {
    let handlers = [
        native("double", [ty::int()], ty::int()),
        native("size", [ty::string()], ty::int()),
        native(
            "apply",
            [ty::function([ty::int()], ty::int()), ty::int()],
            ty::int(),
        ),
        native("refuse", [], ty::int()),
        native("mint", [], ty::named("File")),
    ];
    let main = b::function("main")
        .returns(ty::int())
        .performs(["double", "size", "apply", "refuse", "mint"])
        .body(body);
    let beside = [b::brand("File"), b::target("world").into(), main];
    run_natively(handlers.into_iter().flatten().chain(beside), &natives())
}

#[test]
fn a_bundled_function_reads_an_argument_and_makes_what_it_gives() {
    let body = b::perform("double", [b::int(21)]);

    assert_eq!(outcome(body), Ok(Value::Int(42)));
}

#[test]
fn a_bundle_is_told_of_a_handle_its_program_dropped() {
    let body = b::let_("file", b::perform("mint", []), b::int(42));

    assert_eq!(outcome(body), Ok(Value::Int(42)));
    assert_eq!(CLOSED.load(Ordering::Relaxed), MINTED);
}

#[test]
fn a_bundled_function_reads_a_value_where_it_lies() {
    let body = b::perform("size", [b::string("ambient")]);

    assert_eq!(outcome(body), Ok(Value::Int(7)));
}

#[test]
fn a_bundled_function_calls_a_function_with_what_it_captured() {
    let adds = b::lambda([("n", None)], b::add(b::name("n"), b::name("held")));
    let body = b::perform("apply", [adds, b::int(2)]);

    assert_eq!(
        outcome(b::let_("held", b::int(40), body)),
        Ok(Value::Int(42))
    );
}

#[test]
fn a_fault_in_what_a_bundled_function_calls_goes_on_through_it() {
    let faults = b::lambda([("n", None)], b::divide(b::name("n"), b::int(0)));
    let body = b::perform("apply", [faults, b::int(1)]);

    assert_eq!(outcome(body), Err(Fault::DivideByZero));
}

#[test]
fn a_bundled_function_that_says_it_failed_faults_its_task() {
    let body = b::perform("refuse", []);

    assert_eq!(outcome(body), Err(Fault::Native));
}
