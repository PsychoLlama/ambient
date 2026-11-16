//! Functions that are whatever tree a comptime call gives. See "Splice
//! positions" in `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::value::Value;
use support::text::{int, workspace};
use support::value;

/// A function that makes one that multiplies, and one made with it.
const SCALED: &str = "
    import ambient.code;
    import ambient.tree;
    fn times(left: int, right: int): int => left * right;
    fn scaled(by: int): tree.Function =>
        code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
            code.call(code.definition(comptime tree.reference(times)), given.push(code.literal(by))));
    fn double = comptime scaled(2);
";

/// What is wrong with `main` of a program written after [`SCALED`].
fn refused(main: &str) -> Error {
    let workspace = workspace(&format!("{SCALED} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_function_is_what_a_call_gives() {
    let main = "fn main(): int => double(21);";

    assert_eq!(int(&format!("{SCALED} {main}")), 42);
}

#[test]
fn a_generated_function_is_called_as_its_tree_says() {
    let main = "fn main(): int => double(\"twice\");";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn a_generated_function_has_the_hash_of_the_function_written_out() {
    let written = "fn written(n: int): int => times(n, 2);";
    let workspace = workspace(&format!("{SCALED} {written}"));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |name: &str| engine.hash(&value("main", name)).unwrap();

    assert_eq!(hash("double"), hash("written"));
}

#[test]
fn a_generated_body_is_checked_against_what_the_tree_says_it_gives() {
    let main = "
        fn wrong(): tree.Function =>
            code.define([], tree.text, [], fn(given: List<code.Code>): code.Code => code.literal(1));
        fn main = comptime wrong();
    ";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn a_generated_body_nests_as_deep_as_a_written_one_may() {
    // Each call is inside the last: three hundred levels of code.
    let main = "
        fn nested(): tree.Function =>
            code.define([tree.integer], tree.integer, [], fn(given: List<code.Code>): code.Code =>
                @range(0, 300).fold(given.first() as code.Code else code.literal(0), fn(inner: code.Code, at: int): code.Code =>
                    code.call(code.definition(comptime tree.reference(times)), [inner, code.literal(1)])));
        fn deep = comptime nested();
        fn main(): int => deep(7);
    ";

    assert_eq!(int(&format!("{SCALED} {main}")), 7);
}

/// A function of one integer whose body is `body`, made of the
/// parameter `x`; and `main`, which calls it with seven.
fn of_seven(returns: &str, body: &str) -> String {
    format!(
        "import ambient.code;
        import ambient.tree;
        fn at(codes: List<code.Code>, place: int): code.Code =>
            codes.get(place) as code.Code else code.literal(null);
        fn make(): tree.Function =>
            code.define([tree.integer], tree.{returns}, [], fn(given: List<code.Code>): code.Code {{
                let x = at(given, 0);
                return {body};
            }});
        fn made = comptime make();
        fn main(): unknown => made(7);"
    )
}

#[test]
fn generated_code_does_operations_and_chooses() {
    let sign = "code.choose(
        code.operate(\"less\", [x, code.literal(0)]),
        code.literal(\"below\"),
        code.operate(\"to_text\", [code.operate(\"multiply\", [x, code.literal(6)])]))";

    let ran = support::text::run(&of_seven("text", sign));
    assert_eq!(ran, Ok(Value::String("42".to_owned())));
}

#[test]
fn generated_code_calls_a_method() {
    let shouted = "code.send(code.send(x, \"to_text\", []), \"join\", [code.literal(\"!\")])";

    let ran = support::text::run(&of_seven("text", shouted));
    assert_eq!(ran, Ok(Value::String("7!".to_owned())));
}

#[test]
fn a_generated_match_binds_what_its_patterns_do_in_order() {
    // A tuple's bindings in turn, and a binding before what is inside
    // it: subtraction and the field tell which is which.
    let whole =
        "{ bound: { shape: [{ name: \"less\", matches: { bound: { is: tree.integer } } }] } }";
    let matched = format!(
        "code.matching(code.tuple([x, code.record([(\"less\", code.literal(2))])]), [
            code.arm({{ tuple: [{{ bound: {{ is: tree.integer }} }}, {whole}] }},
                fn(bound: List<code.Code>): code.Code => code.operate(\"subtract\", [
                    code.operate(\"multiply\", [at(bound, 0), code.field(at(bound, 1), \"less\")]),
                    at(bound, 2),
                ])),
        ])"
    );

    let ran = support::text::run(&of_seven("integer", &matched));
    assert_eq!(ran, Ok(Value::Int(12)));
}

#[test]
fn a_tree_that_stands_for_no_code_says_why() {
    // Each is of the type of a tree, and wrong in a way a type cannot
    // say: a local nothing binds, and a value named as an ability.
    let unbound = "{ local: 3 }";
    let miscast = "{ perform: comptime tree.reference(times), given: [] }";
    let reasons = [
        (unbound, "local 3 is named where 1 are bound"),
        (
            miscast,
            "the reference to `times` is of kind `value`, where `ability` is taken",
        ),
    ];
    for (body, reason) in reasons {
        let main = format!(
            "fn wrong(): tree.Function =>
                {{ params: [tree.integer], returns: tree.integer, takes: [], body: {body} }};
            fn made = comptime wrong();
            fn main(): int => made(1);"
        );

        let refused = refused(&main);
        assert!(refused.to_string().ends_with(reason), "{refused}");
    }
}

#[test]
fn a_generated_function_takes_the_abilities_its_tree_lists() {
    let main = "
        ability ask(): int;
        fn asking(): tree.Function =>
            code.define([], tree.integer, [comptime tree.reference(ask)], fn(given: List<code.Code>): code.Code =>
                code.perform(comptime tree.reference(ask), []));
        fn asks = comptime asking();
        fn main(): int => provide { [ask]() => 7 } => asks();
    ";

    assert_eq!(int(&format!("{SCALED} {main}")), 7);
}

#[test]
fn an_ability_is_named_only_to_what_takes_a_reference() {
    let main = "
        ability ask(): int;
        fn main(): int => comptime times(ask, 2);
    ";

    assert!(matches!(refused(main), Error::NotValue { .. }));
}

#[test]
fn a_generated_function_names_only_what_could_be_named_there() {
    let main = "
        fn hidden(): tree.Function =>
            code.define([], tree.any, [], fn(given: List<code.Code>): code.Code =>
                code.definition({ package: \"ambient\", module: [\"map\"], name: \"locate\", kind: \"value\" }));
        fn main = comptime hidden();
    ";

    assert!(matches!(refused(main), Error::Unnameable { .. }));
}

#[test]
fn a_function_cannot_be_generated_from_itself() {
    let main = "
        fn same(of: tree.Type): tree.Function => comptime scaled(1);
        fn main = comptime same(main);
    ";

    assert!(matches!(refused(main), Error::ComptimeCycle(_)));
}

#[test]
fn a_generated_function_follows_what_made_it() {
    let store = MemoryStore::new();
    let hash = |made: &str| {
        let workspace = workspace(made);
        Engine::new(&workspace, &store).hash(&value("main", "double"))
    };
    let (twice, thrice) = (
        hash(SCALED),
        hash(&SCALED.replace("scaled(2)", "scaled(3)")),
    );

    assert!(twice.is_ok() && thrice.is_ok());
    assert_ne!(twice, thrice);
}

/// The code the function `made` of a program is wrong in, as a report
/// shows it.
fn shown(source: &str) -> String {
    let workspace = workspace(source);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let error = engine.check(&value("main", "made")).unwrap_err();
    assert_eq!(engine.blame(&error), None, "no node of it is in the text");
    engine.shown(&error).unwrap()
}

#[test]
fn an_error_in_a_generated_function_is_shown_in_its_code() {
    let sum = "code.operate(\"add\", [x, code.literal(\"two\")])";
    let expected = "    fn made(_0: int): int => _0 + \"two\"
                             ^^^^^^^^^^
";

    assert_eq!(shown(&of_seven("integer", sum)), expected);
}

#[test]
fn generated_code_is_shown_around_the_node_an_error_is_at() {
    // Bindings are named in the order they are made, a block is its
    // lines, and what is too wide for a line is broken over several.
    let long = "a string long enough to push this call past the width of a page";
    let body = format!(
        "code.matching(code.tuple([x, code.literal(2)]), [
            code.arm({{ tuple: [{{ bound: {{ is: tree.integer }} }}, {{ bound: {{ is: tree.any }} }}] }},
                fn(bound: List<code.Code>): code.Code =>
                    code.bind(code.operate(\"add\", [at(bound, 0), at(bound, 1)]), fn(sum: code.Code): code.Code =>
                        code.list([sum, code.field(sum, \"missing\"), code.literal(\"{long}\"), sum, sum]))),
        ])"
    );
    let expected = format!(
        "    fn made(_0: int): int => match (_0, 2) {{
    …
            let _3 = _1 + _2;
            [
                _3,
                _3.missing,
                ^^^^^^^^^^
                \"{long}\",
                _3,
                _3,
    …
"
    );

    assert_eq!(shown(&of_seven("integer", &body)), expected);
}
