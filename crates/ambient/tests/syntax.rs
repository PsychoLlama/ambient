//! Source text, read: what it is as a tree, and what is not text.
//! See `spec/syntax.md`.

#![allow(clippy::unwrap_used)]

use ambient::builder::{self as b, ModuleBuilder, pat, ty};
use ambient::surface::{Definition, Expr, Module};
use ambient::syntax::{Position, parse};

fn built(definitions: impl IntoIterator<Item = Definition>) -> Module {
    let module = definitions
        .into_iter()
        .fold(b::module(), ModuleBuilder::define);
    module.build().unwrap()
}

/// `fn main(): int` with a body.
fn main(body: Expr) -> Definition {
    b::function("main").returns(ty::int()).body(body)
}

/// Where `source` stops being a module, and why.
fn refused(source: &str) -> (usize, usize, String) {
    let error = parse(source).unwrap_err();
    let Position { line, column } = error.at;
    (line, column, error.message)
}

// Trees

#[test]
fn a_function_is_its_header_and_the_expression_its_body_is() {
    let source = "
        /// Adds.
        pub fn add<T: int>(a: T, b: int): int with ask => a + b * 2;
        ability ask(): int;
    ";
    let add = b::function("add")
        .public()
        .doc("Adds.")
        .bounded("T", ty::int())
        .param("a", ty::named("T"))
        .param("b", ty::int())
        .returns(ty::int())
        .performs(["ask"])
        .body(b::add(b::name("a"), b::multiply(b::name("b"), b::int(2))));
    let ask = b::ability("ask").returns(ty::int());

    assert_eq!(parse(source).unwrap(), built([add, ask.into()]));
}

#[test]
fn a_type_that_is_a_comptime_call_is_derived() {
    let source = "type Draft = comptime partial(User);";
    let call = b::call(b::name("partial"), [b::name("User")]);

    assert_eq!(parse(source).unwrap(), built([b::derived("Draft", call)]));
}

#[test]
fn a_function_that_is_a_comptime_call_is_generated() {
    let source = "fn double = comptime scaled(2);";
    let call = b::call(b::name("scaled"), [b::int(2)]);

    assert_eq!(
        parse(source).unwrap(),
        built([b::generated("double", call)])
    );
}

#[test]
fn a_module_that_is_a_comptime_call_is_generated() {
    let source = "import pkg.bind; comptime bind.all();";
    let call = b::call(b::name("bind.all"), []);
    let module = b::module().import("pkg.bind").define(b::generates(call));

    assert_eq!(parse(source).unwrap(), module.build().unwrap());
}

#[test]
fn a_generated_module_is_written_with_nothing_else() {
    let source = "comptime all(); fn more(): int => 1;";

    assert!(parse(source).is_err());
}

#[test]
fn an_operation_with_an_operator_is_also_written_by_its_word() {
    let worded = parse("fn main(): int => @add(1, @multiply(2, 3));").unwrap();
    let infix = parse("fn main(): int => 1 + 2 * 3;").unwrap();

    assert_eq!(worded, infix);
}

#[test]
fn a_block_is_its_statements_nested_in_order() {
    let source = "fn main(): int { let a = 1; side(); let (b, _) = (a, 2); return b; }";
    let pair = pat::tuple([pat::bind("b"), pat::any()]);
    let rest = b::match_(b::tuple([b::name("a"), b::int(2)]), [(pair, b::name("b"))]);
    let body = b::let_(
        "a",
        b::int(1),
        b::let_("_", b::call(b::name("side"), []), rest),
    );

    assert_eq!(parse(source).unwrap(), built([main(body)]));
}

#[test]
fn a_name_is_a_local_or_a_path() {
    // `a.b` is a field of a local and a definition of a module.
    let source = "fn main(a: int): int { return a.b + m.b + a.c(1) + m.c(1); }";
    let local = || b::name("a");
    let body = b::add(
        b::add(
            b::add(b::field(local(), "b"), b::name("m.b")),
            b::send(local(), "c", [b::int(1)]),
        ),
        b::call(b::name("m.c"), [b::int(1)]),
    );
    let main = b::function("main")
        .param("a", ty::int())
        .returns(ty::int())
        .body(body);

    assert_eq!(parse(source).unwrap(), built([main]));
}

#[test]
fn an_early_return_is_a_match_whose_other_arm_is_what_follows() {
    let source = "fn main(): int { if ready() { return 1; } return 2; }";
    let arms = [
        (pat::boolean(true), b::int(1)),
        (pat::boolean(false), b::int(2)),
    ];
    let body = b::match_(b::call(b::name("ready"), []), arms);

    assert_eq!(parse(source).unwrap(), built([main(body)]));
}

#[test]
fn a_bare_name_in_a_pattern_tests_and_let_binds() {
    let source = "fn main(): int => match value() { done => 0, let n: int => n, _ => 1 };";
    let arms = [
        (pat::is(ty::named("done")), b::int(0)),
        (pat::bind_as("n", pat::is(ty::int())), b::name("n")),
        (pat::any(), b::int(1)),
    ];
    let body = b::match_(b::call(b::name("value"), []), arms);

    assert_eq!(parse(source).unwrap(), built([main(body)]));
}

#[test]
fn a_record_is_fields_methods_and_what_it_merges_over() {
    let source = "fn main(): int => { ...base(), x: 1, get(self) => self.x, [ask]() => 2 };";
    let get = b::lambda([("self", None)], b::field(b::name("self"), "x"));
    let fields = b::record([("x", b::int(1)), ("get", b::method(get))]);
    let record = b::keyed(fields, "ask", b::lambda([], b::int(2)));
    let body = b::merge(b::call(b::name("base"), []), record);

    assert_eq!(parse(source).unwrap(), built([main(body)]));
}

#[test]
fn a_type_is_written_as_its_values_are() {
    let source = "
        type Shape<T> = {
            sides: int | null,
            scale(self, by: T): Shape<T>,
            name(): string,
            [label]: string,
        };
        key label: string;
    ";
    let own = ty::apply("Shape", [ty::named("T")]);
    let scale = ty::method(own.clone(), ty::function([ty::named("T")], own));
    let fields = ty::record([
        ("sides", ty::union([ty::int(), ty::null()])),
        ("scale", scale),
        ("name", ty::function([], ty::string())),
    ]);
    let shape = b::generic_type("Shape", ["T"], ty::keyed(fields, "label", ty::string()));

    assert_eq!(
        parse(source).unwrap(),
        built([shape, b::key("label", ty::string())])
    );
}

#[test]
fn imports_name_modules_and_what_is_taken_from_them() {
    let source = "import pkg.a; import pkg.b as c; import pkg.d.{e, f as g}; pub import pkg.h.i;";
    let module = b::module()
        .import("pkg.a")
        .import_as("pkg.b", "c")
        .import("pkg.d.e")
        .import_as("pkg.d.f", "g")
        .export("pkg.h.i");

    assert_eq!(parse(source).unwrap(), module.build().unwrap());
}

#[test]
fn the_world_is_declared_as_externs_and_a_target() {
    let source = "
        extern fn size(text: string): int;
        extern handler write for log.print with log.flush;
        pub target console extends base { log.print: write }
    ";
    let size = b::extern_function("size")
        .param("text", ty::string())
        .returns(ty::int());
    let write = b::native_handler("write", "log.print").performs(["log.flush"]);
    let console = b::target("console")
        .public()
        .extends("base")
        .bind("log.print", "write");

    assert_eq!(
        parse(source).unwrap(),
        built([size.into(), write.into(), console.into()])
    );
}

#[test]
fn a_native_handler_says_whether_it_suspends() {
    let source = "extern suspending handler read for log.read;";
    let read = b::native_handler("read", "log.read").suspending();

    assert_eq!(parse(source).unwrap(), built([read.into()]));
}

// What is not text

#[test]
fn a_statement_ends_where_it_says() {
    let (line, column, message) = refused("fn main(): int {\n  let a = 1\n  return a;\n}");

    assert_eq!((line, column), (3, 3));
    assert_eq!(message, "expected `;`, found `return`");
}

#[test]
fn nothing_follows_a_return() {
    let (line, _, message) = refused("fn main(): int {\n  return 1;\n  side();\n}");

    assert_eq!(
        (line, message.as_str()),
        (2, "what follows a `return` never runs")
    );
}

#[test]
fn no_return_leaves_a_for() {
    let source = "fn main(): int {\n  for n in [1] {\n    return n;\n  }\n  return 0;\n}";
    let (line, _, message) = refused(source);

    assert_eq!(
        (line, message.as_str()),
        (
            2,
            "a `for` runs its block for every item: no `return` leaves it"
        )
    );
}

#[test]
fn a_reserved_word_names_nothing() {
    let (_, _, message) = refused("fn main(): int { let match = 1; return match; }");

    assert_eq!(message, "expected a name, found `match`");
}

#[test]
fn what_a_builder_refuses_the_reader_refuses() {
    let (_, _, message) = refused("fn main(a: int, a: int): int => a;");

    assert!(message.contains('a'), "{message}");
}

#[test]
fn a_string_is_its_characters_with_escapes_read() {
    let source = r#"fn main(): string => "a\"b\n\u{1F600}";"#;
    let main = b::function("main")
        .returns(ty::string())
        .body(b::string("a\"b\n\u{1F600}"));

    assert_eq!(parse(source).unwrap(), built([main]));
}

#[test]
fn a_value_written_into_a_string_is_closed_and_is_something() {
    let (_, column, message) = refused(r#"fn main(): string => "a ${1 + 2"#);
    assert_eq!(
        (column, message.as_str()),
        (25, "a `${` in a string is not closed")
    );

    let (_, _, message) = refused(r#"fn main(): string => "a ${ }";"#);
    assert_eq!(message, "nothing is written between `${` and `}`");
}

#[test]
fn every_arm_of_the_else_of_a_let_leaves() {
    let source = "fn main(n: int | null): int {\n  let m: int = n else { null => { side(); } };\n  return m;\n}";
    let (line, column, message) = refused(source);

    assert_eq!((line, column), (2, 3));
    assert_eq!(
        message,
        "each arm of the `else` of a `let` ends in a `return`"
    );
}

#[test]
fn parentheses_group_one_thing_or_hold_two_or_more() {
    let short = "a tuple has two or more members";

    assert!(parse("fn main(): (int) => (1);").is_ok());
    assert_eq!(refused("fn main(): null => ();"), (1, 20, short.to_owned()));
    assert_eq!(
        refused("fn main(): (int,) => 1;"),
        (1, 12, short.to_owned())
    );
    assert_eq!(
        refused("fn main(): int => match 1 { (n,) => n };"),
        (1, 29, short.to_owned())
    );
}
