//! Opaque types: a type definition only its own module sees into, whose
//! methods are its module's. See "Opaque types" in `docs/spec/types.md`.

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::value::Value;
use support::text::modules;
use support::{run_in, value};

/// A counter, opaque over an int, and a box, opaque over a record.
const COUNTER: &str = "
    pub type(opaque) Counter = int;
    pub fn start(): Counter => 0;
    pub fn bump(self: Counter): Counter => self + 1;
    pub fn count(self: Counter): int => self;

    pub type(opaque) Box = { held: int };
    pub fn boxed(held: int): Box => { held };
    pub fn held(self: Box): int => self.held;
";

/// `main` written beside `counter`, which it imports.
fn beside(main: &str) -> String {
    format!("use pkg.counter.{{self, Counter, Box}};\n{main}")
}

/// What `main` gives, written beside `counter`.
fn outcome(main: &str) -> Option<Value> {
    run_in(&modules(&[("counter", COUNTER), ("main", &beside(main))])).ok()
}

/// What is wrong with `main`, written beside `counter`, if anything is.
fn wrong(main: &str) -> Option<Error> {
    let workspace = modules(&[("counter", COUNTER), ("main", &beside(main))]);
    let store = MemoryStore::new();
    Engine::new(&workspace, &store)
        .check(&value("main", "main"))
        .err()
}

#[test]
fn an_opaque_type_has_the_methods_of_its_module() {
    let main = "fn main(): int => counter.start().bump().bump().count() + counter.boxed(3).held();";
    assert_eq!(outcome(main), Some(Value::Int(5)));
}

#[test]
fn an_opaque_type_has_no_other_methods() {
    let main = "fn main(): int => counter.start().negate().count();";
    // `boxed` is of the module, but takes an int first.
    let other = "fn main(): Box => counter.start().boxed();";
    assert!(matches!(wrong(main), Some(Error::NoMethod { .. })));
    assert!(matches!(wrong(other), Some(Error::NoMethod { .. })));
}

#[test]
fn outside_its_module_an_opaque_type_is_not_its_content() {
    let added = "fn main(): int => counter.start() + 1;";
    let written = "fn main(): int => counter.count(5);";
    let read = "fn main(): int => counter.boxed(3).held;";
    let spread = "fn main(): Box => { ...counter.boxed(3), held: 4 };";
    assert!(wrong(added).is_some());
    assert!(matches!(wrong(written), Some(Error::Mismatch { .. })));
    assert!(wrong(read).is_some());
    assert!(wrong(spread).is_some());
}

#[test]
fn two_opaque_types_of_one_content_are_two_types() {
    let source = "
        pub type(opaque) Other = int;
        pub fn other(): Other => 0;
    ";
    let main = "use pkg.other; fn main(): Counter => other.other();";
    let workspace = modules(&[
        ("counter", COUNTER),
        ("other", source),
        ("main", &beside(main)),
    ]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    assert!(matches!(checked, Err(Error::Mismatch { .. })));
}

#[test]
fn an_opaque_type_is_tested_for_where_its_content_tells_it_apart() {
    let main = "
        fn read(held: Counter | string): int => match held {
            let counted: Counter => counted.count(),
            let text: string => text.length(),
        };
        fn main(): int => read(counter.start().bump()) + read(\"ab\");
    ";
    assert_eq!(outcome(main), Some(Value::Int(3)));
}

#[test]
fn nothing_is_cast_to_an_opaque_type_its_content_cannot_tell() {
    let cast = "fn main(): int => cast::<Counter>(1, counter.start()).count();";
    let overlapping = "
        fn main(held: Counter | int): int => match held {
            let counted: Counter => counted.count(),
            _ => 0,
        };
    ";
    assert!(matches!(wrong(cast), Some(Error::UntestableOpaque { .. })));
    assert!(matches!(
        wrong(overlapping),
        Some(Error::UntestableOpaque { .. })
    ));
}

#[test]
fn making_a_type_opaque_checks_what_uses_it_again() {
    let store = MemoryStore::new();
    let main = beside("fn main(): int => counter.start() + 1;");
    let transparent = COUNTER.replace("type(opaque) Counter", "type Counter");
    let before = modules(&[("counter", &transparent), ("main", &main)]);
    let after = modules(&[("counter", COUNTER), ("main", &main)]);
    let checked = |workspace| Engine::new(workspace, &store).check(&value("main", "main"));
    assert!(checked(&before).is_ok());
    assert!(checked(&after).is_err());
}
