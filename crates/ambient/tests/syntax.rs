//! Source text, read: what it is as a tree, and what is not text.
//! See `docs/spec/syntax.md`.

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
    let source = "use pkg.bind; comptime bind.all();";
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
        contract label: string;
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
        built([shape, b::contract("label", ty::string())])
    );
}

#[test]
fn imports_name_modules_and_what_is_taken_from_them() {
    let source = "use pkg.a; use pkg.b as c; use pkg.d.{e, f as g}; pub use pkg.h.i;";
    let module = b::module()
        .import("pkg.a")
        .import_as("pkg.b", "c")
        .import("pkg.d.e")
        .import_as("pkg.d.f", "g")
        .export("pkg.h.i");

    assert_eq!(parse(source).unwrap(), module.build().unwrap());
}

#[test]
fn a_group_of_imports_nests_and_names_what_it_is_under() {
    let source = "use pkg.{a, b.{self, c as d, e.{f}}};";
    let module = b::module()
        .import("pkg.a")
        .import("pkg.b")
        .import_as("pkg.b.c", "d")
        .import("pkg.b.e.f");

    assert_eq!(parse(source).unwrap(), module.build().unwrap());
}

#[test]
fn an_annotation_is_a_key_and_a_value_or_a_key_alone() {
    let source = r#"#[note("a")] #[pkg.flag] fn main(): int => 1;"#;
    let main = main(b::int(1))
        .annotated("note", b::string("a"))
        .annotated("pkg.flag", Expr::Null);

    assert_eq!(parse(source).unwrap(), built([main]));
}

#[test]
fn the_world_is_declared_as_native_handlers_and_a_target() {
    let source = "
        extern handler write for log.print with log.flush;
        pub target console extends base { log.print: write }
    ";
    let write = b::native_handler("write", "log.print").performs(["log.flush"]);
    let console = b::target("console")
        .public()
        .extends("base")
        .bind("log.print", "write");

    assert_eq!(
        parse(source).unwrap(),
        built([write.into(), console.into()])
    );
}

#[test]
fn native_code_is_declared_only_as_a_handler() {
    assert!(parse("extern fn size(text: string): int;").is_err());
}

#[test]
fn a_native_handler_says_whether_it_suspends() {
    let source = "extern handler(suspending) read for log.read;";
    let read = b::native_handler("read", "log.read").suspending();

    assert_eq!(parse(source).unwrap(), built([read.into()]));
}

#[test]
fn a_native_handler_takes_its_abilitys_name_and_says_where_it_is_a_default() {
    let source = "extern handler log.read with log.flush in os, web.page;";
    let read = b::native_handler("read", "log.read")
        .performs(["log.flush"])
        .under("os")
        .under("web.page");

    assert_eq!(parse(source).unwrap(), built([read.into()]));
}

#[test]
fn what_is_said_of_a_kind_follows_its_keyword_in_brackets() {
    let source = "pub type(nominal) Meters;";

    assert_eq!(parse(source).unwrap(), built([b::brand("Meters").public()]));
}

#[test]
fn a_provide_is_restricted_by_the_word_and_by_no_bundle_in_brackets() {
    let source = "fn main(): int {
        let bundle = {};
        return provide(restricted) bundle => provide (bundle) => 1;
    }";
    let inner = b::with(b::name("bundle"), b::int(1));
    let outer = b::with_restricted(b::name("bundle"), inner);

    assert_eq!(
        parse(source).unwrap(),
        built([main(b::let_("bundle", b::record([]), outer))])
    );
}

#[test]
fn a_name_in_a_type_is_for_a_reader_and_is_no_part_of_the_type() {
    let source = "type Found = fn(note: string, at: (line: int, column: int)): null;";
    let at = ty::tuple([ty::int(), ty::int()]);
    let found = ty::function([ty::string(), at], ty::null());

    assert_eq!(parse(source).unwrap(), built([b::type_("Found", found)]));
}

#[test]
fn a_type_without_its_names_is_refused() {
    let (_, _, tuple) = refused("type Pair = (string, int);");
    let (_, _, function) = refused("type Found = fn(string): null;");

    assert_eq!(tuple, "a member of a tuple type is named, as `count: int`");
    assert_eq!(function, "a parameter in a type is named, as `count: int`");
}

#[test]
fn a_negated_base_of_a_power_is_refused() {
    let said = "`-` before the base of `**` could negate the base or the power: \
                write `(-x) ** y` or `-(x ** y)`";

    for written in ["-2 ** 2", "-x ** 2"] {
        let (_, _, message) = refused(&format!("fn main(x: int): int => {written};"));
        assert_eq!(message, said, "{written}");
    }
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

    assert_eq!(message, "expected a name, found `match`, which is reserved");
}

#[test]
fn a_function_type_that_a_function_gives_is_written_in_parentheses() {
    let bare = [
        "fn main(): fn(): null with throw => 1;",
        "fn main(): int | fn(): null => 1;",
        "fn main(make: fn(): fn(): null): int => 1;",
        "fn main(): int => (fn(): fn(): null => 1)();",
    ];

    for source in bare {
        let (_, _, message) = refused(source);
        assert!(
            message.starts_with("a function type that a function gives"),
            "{message}"
        );
    }
}

#[test]
fn a_function_type_inside_what_a_function_gives_is_written_bare() {
    let source = "fn main(): List<fn(): null> with throw => [];
        fn other(): (run: fn(): null, count: int) => (fn() => null, 1);
        fn third(): { run: fn(): null } => { run: fn() => null };";

    assert!(parse(source).is_ok(), "{:?}", parse(source).err());
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
fn statements_in_the_else_of_a_let_are_told_to_be_arms() {
    let source =
        "fn main(n: int | null): int {\n  let m: int = n else { return 0; };\n  return m;\n}";
    let (line, column, message) = refused(source);

    assert_eq!((line, column), (2, 25));
    assert_eq!(
        message,
        "the `else` of a `let` holds arms, as `_ => { return; }`"
    );
}

#[test]
fn an_arm_binds_what_it_tests_as_a_tuple_type() {
    let arm = "fn main(v: unknown): int => match v { let at: (x: int, y: int) => at.x, _ => 0 };";
    let tuple = "fn main(v: (a: int, b: int)): int => match v { (1, int) => 1, _ => 0 };";

    assert!(parse(arm).is_ok());
    assert!(parse(tuple).is_ok());
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

#[test]
fn a_constant_is_its_name_its_type_and_its_value() {
    let limit = b::constant("LIMIT_2", ty::int()).public().value(b::int(7));
    assert_eq!(
        parse("pub const LIMIT_2: int = 7;").unwrap(),
        built([limit])
    );
}

#[test]
fn a_constant_not_named_in_capitals_is_refused_at_its_name() {
    let (line, column, message) = refused("const Limit: int = 7;");
    assert_eq!((line, column), (1, 12));
    assert!(message.contains("capital letters"));
}

#[test]
fn const_names_anything_where_no_definition_begins() {
    let body = b::let_("const", b::int(1), b::name("const"));
    let source = "fn main(): int { let const = 1; return const; }";
    assert_eq!(parse(source).unwrap(), built([main(body)]));
}
