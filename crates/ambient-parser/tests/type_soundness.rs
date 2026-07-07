//! Regression tests for type-checker soundness holes.
//!
//! Each test pins a bug where the checker previously accepted an unsound
//! program (or produced code the compiler could not handle). These check
//! whole programs through `parse` + `check_module`, the same path `ambient
//! check` takes; user-declared abilities stand in for platform ones so no
//! embedder wiring is needed.

use ambient_engine::infer::check_module;

fn check(source: &str) -> Vec<String> {
    let module = ambient_parser::parse(source).expect("test source must parse");
    let result = check_module(module);
    result
        .errors
        .iter()
        .map(std::string::ToString::to_string)
        .collect()
}

fn assert_ok(source: &str) {
    let errors = check(source);
    assert!(errors.is_empty(), "expected no errors, got: {errors:#?}");
}

fn assert_err_containing(source: &str, needle: &str) {
    let errors = check(source);
    assert!(
        errors.iter().any(|e| e.contains(needle)),
        "expected an error containing {needle:?}, got: {errors:#?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait impl purity
// ─────────────────────────────────────────────────────────────────────────────

/// Trait method signatures carry no `with` clause, so impl bodies must be
/// pure. Previously their inferred effects were silently discarded and a
/// pure-declared public function could print through a trait method.
#[test]
fn trait_impl_method_performing_ability_is_rejected() {
    assert_err_containing(
        &r"
        ability Printer {
          fn print(msg: String): ();
        }

        trait Show {
          fn show(self): String;
        }

        unique(11111111-1111-1111-1111-111111111111) struct Money { cents: Number }

        impl Show for Money {
          fn show(self): String {
            Printer::print!('effect');
            'money'
          }
        }

        pub fn run(): () {
          let m = Money { cents: 100 };
          let s = m.show();
          ()
        }
        "
        .replace('\'', "\""),
        "ability",
    );
}

#[test]
fn pure_trait_impl_method_is_accepted() {
    assert_ok(
        &r"
        trait Show {
          fn show(self): String;
        }

        unique(11111111-1111-1111-1111-111111111111) struct Money { cents: Number }

        impl Show for Money {
          fn show(self): String { 'money' }
        }

        pub fn run(): String {
          Money { cents: 100 }.show()
        }
        "
        .replace('\'', "\""),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler arm parameter types
// ─────────────────────────────────────────────────────────────────────────────

/// Handler arm parameters take the ability method's declared types.
/// Previously they were fresh unconstrained variables, so treating a
/// thrown string as a number type-checked.
#[test]
fn handler_arm_param_takes_declared_type() {
    assert_err_containing(
        &r"
        fn boom(): Number with Exception {
          Exception::throw!('boom');
          1
        }

        pub fn run(): Number {
          with { Exception::throw(e) => e * 2 } handle boom()
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

#[test]
fn handler_arm_param_usable_at_declared_type() {
    assert_ok(
        &r"
        fn boom(): String with Exception {
          Exception::throw!('boom');
          'unreachable'
        }

        pub fn run(): String {
          with { Exception::throw(e) => e + '!' } handle boom()
        }
        "
        .replace('\'', "\""),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Resume typing
// ─────────────────────────────────────────────────────────────────────────────

/// `resume` feeds the ability method's return type to the continuation.
/// Previously the value was type-checked but never constrained.
#[test]
fn resume_value_must_match_method_return_type() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }

        fn get(): Number with Reader {
          Reader::read!()
        }

        pub fn run(): Number {
          with { Reader::read() => resume('not a number') } handle get()
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

#[test]
fn resume_with_correct_type_is_accepted() {
    assert_ok(
        &r"
        ability Reader {
          fn read(): Number;
        }

        fn get(): Number with Reader {
          Reader::read!()
        }

        pub fn run(): Number {
          with { Reader::read() => resume(42) } handle get()
        }
        "
        .replace('\'', "\""),
    );
}

/// The resume expression itself has the handle expression's result type,
/// so an arm can return `resume(...)` even when the handle result is not
/// unit.
#[test]
fn resume_expression_takes_handle_result_type() {
    assert_ok(
        &r"
        ability Reader {
          fn read(): Number;
        }

        fn get(): Number with Reader {
          Reader::read!() + 1
        }

        pub fn run(): Number {
          with { Reader::read() => resume(41) } handle get()
        }
        "
        .replace('\'', "\""),
    );
}

#[test]
fn resume_outside_handler_is_rejected() {
    assert_err_containing(
        &r"
        pub fn run(): Number {
          resume(1);
          2
        }
        "
        .replace('\'', "\""),
        "resume",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler arm effects propagate
// ─────────────────────────────────────────────────────────────────────────────

/// Effects performed by a handler arm run outside the delimited body and
/// must count against the enclosing function. Previously they were
/// silently dropped when the saved accumulator was restored.
#[test]
fn handler_arm_effects_flow_to_enclosing_function() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }
        ability Printer {
          fn print(msg: String): ();
        }

        fn get(): Number with Reader {
          Reader::read!()
        }

        pub fn run(): Number {
          with {
            Reader::read() => {
              Printer::print!('arm effect');
              resume(1)
            }
          } handle get()
        }
        "
        .replace('\'', "\""),
        "Printer",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Handle discharges polymorphic body effects
// ─────────────────────────────────────────────────────────────────────────────

/// A handle whose body calls a private function declared *later* sees only
/// an unbound effect variable at the handle site. The discharge must still
/// happen once that variable binds — previously the handled ability leaked
/// upward and pure callers got spurious missing-ability errors.
#[test]
fn handle_discharges_effects_of_functions_declared_later() {
    assert_ok(
        &r"
        ability Reader {
          fn read(): Number;
        }

        pub fn run(): Number {
          with { Reader::read() => resume(7) } handle helper()
        }

        fn helper(): Number with Reader {
          Reader::read!() + 1
        }
        "
        .replace('\'', "\""),
    );
}

/// The discharge must not over-subtract: an unhandled ability still
/// escapes the handle and is reported.
#[test]
fn handle_does_not_discharge_unhandled_abilities() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }
        ability Printer {
          fn print(msg: String): ();
        }

        pub fn run(): Number {
          with { Reader::read() => resume(7) } handle helper()
        }

        fn helper(): Number with Reader, Printer {
          Printer::print!('leak');
          Reader::read!()
        }
        "
        .replace('\'', "\""),
        "Printer",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler values — answer type (`Handler<A, R>`) and single-ability rule
// ─────────────────────────────────────────────────────────────────────────────

/// A named handler value's non-resume arm return type is now checked against
/// the handle result (the "answer" type `R`). Previously it was unchecked, so
/// a handler that answered a `String` could be installed at a `Number` handle.
#[test]
fn handler_value_non_resume_arm_must_match_handle_result() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }

        fn get(): Number with Reader {
          Reader::read!()
        }

        pub fn run(): Number {
          let h = { Reader::read() => 'a string' };
          with h handle get()
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

/// A resuming-only handler value stays polymorphic in its answer type `R`, so
/// the same value installs at two handles with different result types.
#[test]
fn resuming_handler_value_is_answer_polymorphic() {
    assert_ok(
        &r"
        ability Ask {
          fn ask(): Number;
        }

        fn n(): Number with Ask {
          Ask::ask!()
        }

        fn s(): String with Ask {
          Ask::ask!();
          'text'
        }

        pub fn run(): () {
          let h = { Ask::ask() => resume(0) };
          let a = with h handle n();
          let b = with h handle s();
          ()
        }
        "
        .replace('\'', "\""),
    );
}

/// A multi-ability brace is legal directly inline in a `with` list: it
/// desugars into one single-ability install per ability.
#[test]
fn multi_ability_inline_brace_is_accepted() {
    assert_ok(
        &r"
        ability Reader {
          fn read(): Number;
        }
        ability Printer {
          fn print(msg: String): ();
        }

        fn body(): Number with Reader, Printer {
          Printer::print!('x');
          Reader::read!()
        }

        pub fn run(): Number {
          with { Reader::read() => resume(7), Printer::print(m) => resume(()) } handle body()
        }
        "
        .replace('\'', "\""),
    );
}

/// A multi-ability brace used where a *value* is expected (a `let` binding) is
/// a type error: a handler value covers one ability.
#[test]
fn multi_ability_handler_value_is_rejected() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }
        ability Printer {
          fn print(msg: String): ();
        }

        pub fn run(): Number {
          let h = { Reader::read() => resume(7), Printer::print(m) => resume(()) };
          1
        }
        "
        .replace('\'', "\""),
        "one ability",
    );
}

/// The ability named in a `Handler<A, R>` annotation resolves under the
/// same namespace policy as every other ability position — the qualifier
/// is not blindly stripped to its last segment. A local ability resolves
/// bare; naming it under a bogus namespace is an unknown reference, not a
/// silent match on the tail segment.
#[test]
fn handler_annotation_ability_obeys_namespace_policy() {
    let program = |ability_ref: &str| {
        format!(
            r"
            ability Reader {{
              fn read(): Number;
            }}

            fn get(): Number with Reader {{
              Reader::read!()
            }}

            pub fn install(h: Handler<{ability_ref}>): Number {{
              with h handle get()
            }}
            "
        )
    };

    // Bare local ability: accepted.
    assert_ok(&program("Reader"));

    // A bogus namespace no longer strips to `Reader`: it is unknown.
    assert_err_containing(&program("bogus::Reader"), "unknown ability");
}

// ─────────────────────────────────────────────────────────────────────────────
// Perform argument checking
// ─────────────────────────────────────────────────────────────────────────────

/// Builtin descriptor abilities check arguments too: Exception::throw
/// takes a string. Previously the builtin path resolved only the return
/// type and ignored arguments entirely.
#[test]
fn builtin_perform_arguments_are_checked() {
    assert_err_containing(
        &r"
        pub fn run(): Number with Exception {
          Exception::throw!(42);
          1
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

/// Arguments to a perform are unified against the declared parameter
/// types of the ability method.
#[test]
fn perform_arguments_are_checked() {
    assert_err_containing(
        &r"
        ability Printer {
          fn print(msg: String): ();
        }

        pub fn run(): () with Printer {
          Printer::print!(42)
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Sandbox
// ─────────────────────────────────────────────────────────────────────────────

/// The sandbox installs no handlers — the body runs against the enclosing
/// context's handlers — so its effects must count against the enclosing
/// function. Previously they were dropped entirely, letting a pure-declared
/// function smuggle effects through `sandbox with X { ... }`.
#[test]
fn sandbox_body_effects_flow_to_enclosing_function() {
    assert_err_containing(
        &r"
        ability Printer {
          fn print(msg: String): ();
        }

        pub fn run(): () {
          sandbox with Printer {
            Printer::print!('hi')
          }
        }
        "
        .replace('\'', "\""),
        "Printer",
    );
}

/// The restriction applies even when the body's effect set is polymorphic
/// at the sandbox site (a call to a function declared later). Previously
/// non-concrete sets bypassed the check entirely.
#[test]
fn sandbox_restriction_applies_to_functions_declared_later() {
    assert_err_containing(
        &r"
        ability Reader {
          fn read(): Number;
        }
        ability Printer {
          fn print(msg: String): ();
        }

        pub fn run(): Number with Reader, Printer {
          sandbox with Reader {
            helper()
          }
        }

        fn helper(): Number with Reader, Printer {
          Printer::print!('not allowed in sandbox');
          Reader::read!()
        }
        "
        .replace('\'', "\""),
        "sandbox",
    );
}

#[test]
fn sandbox_with_allowed_abilities_is_accepted() {
    assert_ok(
        &r"
        ability Reader {
          fn read(): Number;
        }

        pub fn run(): Number with Reader {
          sandbox with Reader {
            helper()
          }
        }

        fn helper(): Number with Reader {
          Reader::read!()
        }
        "
        .replace('\'', "\""),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// `with` clause resolution
// ─────────────────────────────────────────────────────────────────────────────

/// A typo'd ability name in a `with` clause is an error. Previously it
/// was silently dropped, declaring the function pure.
#[test]
fn unknown_ability_in_with_clause_is_reported() {
    assert_err_containing(
        &r"
        pub fn run(): () with Consoel {
          ()
        }
        "
        .replace('\'', "\""),
        "unknown ability",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Generalization
// ─────────────────────────────────────────────────────────────────────────────

/// The classic HM bug: generalizing a let must not quantify variables that
/// are only reachable from the environment through the substitution.
/// `f` is bound to the lambda's parameter type via unification; the inner
/// `let g = ...` must not generalize over it and allow using `x` at two
/// different types.
#[test]
fn generalization_respects_substituted_env_vars() {
    assert_err_containing(
        &r"
        fn apply_twice(): Number {
          let f = (x) => {
            let g = x;
            g + 1;
            if g { 2 } else { 3 }
          };
          f(1)
        }
        "
        .replace('\'', "\""),
        "type mismatch",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// `extern` structs — engine-provided types, unconstructable by user code
// ─────────────────────────────────────────────────────────────────────────────

/// A field-bearing `extern` struct may be named and read, but constructing it
/// with `{ field: value }` is a type error — only the engine builds its values.
#[test]
fn extern_struct_field_construction_is_rejected() {
    assert_err_containing(
        r"
        extern unique(A1B2C3D4-0000-0000-0000-000000000001) struct Handle { id: Number }
        fn make(): Handle { Handle { id: 1 } }
        ",
        "provided by the engine",
    );
}

/// Naming an `extern` struct in a type position and reading a field off a value
/// of that type must still work — the ban is on construction only.
#[test]
fn extern_struct_field_read_is_accepted() {
    assert_ok(
        r"
        extern unique(A1B2C3D4-0000-0000-0000-000000000001) struct Handle { id: Number }
        fn describe(h: Handle): Number { h.id }
        ",
    );
}

/// A bare `extern` unit struct is a type but not a value: using its name in
/// value position is an undefined value, not a construction.
#[test]
fn extern_unit_struct_bare_value_is_rejected() {
    let errors = check(
        r"
        extern unique(A1B2C3D4-0000-0000-0000-000000000002) struct Token;
        fn get(): Token { Token }
        ",
    );
    assert!(
        !errors.is_empty(),
        "expected an error for bare extern unit struct in value position"
    );
}
