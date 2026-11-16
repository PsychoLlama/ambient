//! Modules whose whole content is a comptime call. See "Splice
//! positions" in `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::{Engine, Stage};
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::value::Value;
use ambient::workspace::Workspace;
use support::text::modules;
use support::value;

/// A module of what makes definitions, with `all` for what it gives.
fn make(all: &str) -> String {
    format!(
        "import ambient.code;
        import ambient.tree;
        pub fn times(left: int, right: int): int => left * right;
        pub fn scaled(by: int): tree.Function =>
            code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
                code.call(code.definition(comptime tree.reference(times)), given.push(code.literal(by))));
        pub fn all(): tree.Module => {all};"
    )
}

/// A program of three modules: `make`, which gives `all`; `made`,
/// which is the call of it; and `main`.
fn program(all: &str, main: &str) -> Workspace {
    let made = "import pkg.make; comptime make.all();";
    modules(&[("make", &make(all)), ("made", made), ("main", main)])
}

const BOTH: &str = "[
    code.named(\"double\", scaled(2)),
    code.named(\"triple\", scaled(3)),
    code.named_type(\"Count\", tree.integer),
]";

/// What is wrong with `main` of a program whose module is `all`.
fn refused(all: &str, main: &str) -> Error {
    let workspace = program(all, main);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_module_has_the_definitions_its_call_gives() {
    let main = "import pkg.made; fn main(): made.Count => made.double(made.triple(7));";

    assert_eq!(support::run_in(&program(BOTH, main)), Ok(Value::Int(42)));
}

#[test]
fn what_a_call_gives_is_imported_by_name() {
    let main = "import pkg.made.{double, Count}; fn main(): Count => double(4);";

    assert_eq!(support::run_in(&program(BOTH, main)), Ok(Value::Int(8)));
}

#[test]
fn a_generated_definition_has_the_hash_of_one_written_out() {
    let main = "import pkg.make.{times}; fn written(n: int): int => times(n, 2);";
    let workspace = program(BOTH, main);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |module: &str, name: &str| engine.hash(&value(module, name)).unwrap();

    assert_eq!(hash("made", "double"), hash("main", "written"));
}

#[test]
fn a_call_gives_abilities_and_what_performs_them() {
    let all = "[
        code.named_ability(\"ask\", [tree.integer], tree.integer),
        code.named(\"asked\", code.define([], tree.integer, [asks()], fn(given: List<code.Code>): code.Code =>
            code.perform(asks(), [code.literal(20)]))),
    ]";
    let asks = "fn asks(): tree.Reference =>
        { package: \"app\", module: [\"made\"], name: \"ask\", kind: \"ability\" };";
    let call = "import pkg.make; comptime make.all();";
    let main = "import pkg.made;
        fn main(): int => provide { [made.ask](n) => n + 1 } => made.asked() + made.ask!(20);";
    let maker = format!("{} {asks}", make(all));
    let workspace = modules(&[("make", &maker), ("made", call), ("main", main)]);

    assert_eq!(support::run_in(&workspace), Ok(Value::Int(42)));
}

#[test]
fn a_call_gives_tags() {
    let all = "[code.named_tag(\"ready\"), code.named_tag(\"spent\")]";
    let main = "import pkg.made;
        fn state(): made.ready | made.spent => made.ready;
        fn main(): bool => state() == made.ready && state() != made.spent;";

    assert_eq!(support::run_in(&program(all, main)), Ok(Value::Bool(true)));
}

#[test]
fn a_module_has_nothing_its_call_did_not_give() {
    let main = "import pkg.made; fn main(): int => made.halve(4);";

    assert!(matches!(refused(BOTH, main), Error::UnknownName { .. }));
}

#[test]
fn what_is_not_public_stays_in_its_module() {
    let all = "[{ name: \"double\", public: false, function: scaled(2) }]";
    let main = "import pkg.made; fn main(): int => made.double(4);";

    assert!(matches!(refused(all, main), Error::PrivateName { .. }));
}

#[test]
fn a_call_gives_one_definition_of_a_name() {
    let all = "[code.named(\"double\", scaled(2)), code.named(\"double\", scaled(3))]";
    let main = "import pkg.made; fn main(): int => made.double(4);";

    assert!(matches!(refused(all, main), Error::GeneratedTwice { .. }));
}

#[test]
fn a_generated_module_names_only_what_could_be_named_there() {
    let hidden = "{ package: \"ambient\", module: [\"map\"], name: \"locate\", kind: \"value\" }";
    let all = format!(
        "[code.named(\"reach\", code.define([], tree.any, [], fn(given: List<code.Code>): code.Code =>
            code.definition({hidden})))]"
    );
    let main = "import pkg.made; fn main(): unknown => made.reach();";

    assert!(matches!(refused(&all, main), Error::Unnameable { .. }));
}

#[test]
fn a_call_cannot_depend_on_what_it_gives() {
    let call = "import pkg.make; comptime make.all();";
    let maker = format!(
        "import pkg.made; {}",
        make("[code.named(\"again\", scaled(made.double(1)))]")
    );
    let main = "import pkg.made; fn main(): int => made.again(4);";
    let workspace = modules(&[("make", &maker), ("made", call), ("main", main)]);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));

    assert!(matches!(checked, Err(Error::ComptimeCycle(_))));
}

#[test]
fn the_module_of_a_call_imports_what_the_call_gives() {
    let maker = format!(
        "import pkg.made; import pkg.made.{{triple}};
        pub fn six(): int => made.double(triple(1)); {}",
        make(BOTH)
    );
    let call = "import pkg.make; comptime make.all();";
    // The generated module is asked for first, and then by the module
    // its call is in.
    let main = "import pkg.made; import pkg.make;
        fn main(): int => made.double(1) + make.six();";
    let workspace = modules(&[("make", &maker), ("made", call), ("main", main)]);

    assert_eq!(support::run_in(&workspace), Ok(Value::Int(8)));
}

#[test]
fn a_generated_module_is_made_once_for_a_store() {
    let main = "import pkg.made; fn main(): int => made.double(21);";
    let workspace = program(BOTH, main);
    let store = MemoryStore::new();
    let build = || {
        let engine = Engine::new(&workspace, &store);
        engine.check(&value("main", "main")).unwrap();
        engine.ran(Stage::Check)
    };

    assert!(!build().is_empty());
    assert_eq!(build(), Vec::<String>::new());
}
