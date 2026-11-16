//! Types handed to comptime functions as trees. See "Trees as values"
//! in `spec/comptime.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use ambient::symbol::ModuleId;
use ambient::syntax::parse;
use support::text::{int, text, workspace};
use support::value;

/// A function that reads a record type, and a type for it to read.
const FIELDS: &str = "
    import ambient.tree;
    type User = { name: string, age: int };
    fn fields(of: tree.Type): string {
        match of {
            let shape: { record: List<tree.Field> } => {
                return shape.record.map(fn(field) => field.name).join_with(\", \");
            }
            _ => { return \"not a record\"; }
        }
    }
";

/// What is wrong with `main` of a program written after [`FIELDS`].
fn refused(main: &str) -> Error {
    let workspace = workspace(&format!("{FIELDS} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_function_reads_the_fields_of_a_type_it_is_handed() {
    let main = "fn main(): string => comptime fields(User);";

    assert_eq!(text(&format!("{FIELDS} {main}")), "age, name");
}

#[test]
fn the_result_is_a_constant_of_the_program() {
    let main = "fn main(): \"age, name\" => comptime fields(User);";

    assert_eq!(text(&format!("{FIELDS} {main}")), "age, name");
}

#[test]
fn a_type_is_a_tree_all_the_way_down() {
    let source = "
        import ambient.tree;
        type Shape = { sides: int | null, names: List<string> };
        fn count(of: tree.Type): int {
            match of {
                let it: { record: List<tree.Field> } => {
                    return it.record.fold(1, fn(sum, field) => sum + count(field.holds));
                }
                let it: { union: List<tree.Type> } => {
                    return it.union.fold(1, fn(sum, member) => sum + count(member));
                }
                let it: { list: tree.Type } => { return 1 + count(it.list); }
                _ => { return 1; }
            }
        }
        fn main(): int => comptime count(Shape);
    ";

    // The record, a union of two, and a list of one.
    assert_eq!(int(source), 6);
}

#[test]
fn other_arguments_are_checked_as_those_of_any_call_are() {
    let main = "
        fn has(of: tree.Type, name: string): bool => fields(of).contains(name);
        fn main(): bool => comptime has(User, 1);
    ";

    assert!(matches!(refused(main), Error::Mismatch { .. }));
}

#[test]
fn only_the_name_of_a_type_is_handed_as_a_tree() {
    let main = "fn main(): string => comptime fields(1);";

    assert!(matches!(refused(main), Error::NoTree { .. }));
}

#[test]
fn a_type_that_refers_to_itself_is_named_where_it_does() {
    let source = "
        import ambient.tree;
        type Chain = { next: Chain | null };
        fn named(of: tree.Type): string {
            match of {
                let it: { record: List<tree.Field> } => {
                    return it.record.map(fn(field) => named(field.holds)).join_with(\" \");
                }
                let it: { union: List<tree.Type> } => {
                    return it.union.map(named).join_with(\"|\");
                }
                let it: { named: tree.Reference } => { return it.named.name; }
                _ => { return \"-\"; }
            }
        }
        fn main(): string => comptime named(Chain);
    ";

    assert_eq!(text(source), "-|Chain");
}

#[test]
fn a_result_is_not_reused_once_the_type_has_changed() {
    let main = "fn main(): \"age, name\" => comptime fields(User);";
    let wider = FIELDS.replace("age: int", "age: int, email: string");
    let store = MemoryStore::new();
    let check = |fields: &str| {
        let workspace = workspace(&format!("{fields} {main}"));
        Engine::new(&workspace, &store)
            .check(&value("main", "main"))
            .map(|_| ())
    };

    assert_eq!(check(FIELDS), Ok(()));
    assert!(matches!(check(&wider), Err(Error::Mismatch { .. })));
}

/// A function that gives a record type with every field optional.
const PARTIAL: &str = "
    import ambient.tree;
    type User = { name: string, age: int };
    fn partial(of: tree.Type): tree.Type {
        match of {
            let shape: { record: List<tree.Field> } => {
                return { record: shape.record.map(fn(field) => {
                    name: field.name,
                    holds: { union: [field.holds, tree.nil] },
                }) };
            }
            _ => { return of; }
        }
    }
    type Draft = comptime partial(User);
";

/// What is wrong with `main` of a program written after [`PARTIAL`].
fn undrafted(main: &str) -> Error {
    let workspace = workspace(&format!("{PARTIAL} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_type_is_what_a_call_gives() {
    let main = "
        fn named(draft: Draft): string {
            match draft.name {
                null => { return \"nobody\"; }
                let name => { return name; }
            }
        }
        fn main(): string => named({ name: null, age: 3 });
    ";

    assert_eq!(text(&format!("{PARTIAL} {main}")), "nobody");
}

#[test]
fn a_derived_type_is_checked_as_one_written_out() {
    let main = "fn main(): Draft => { name: 1, age: null };";

    assert!(matches!(undrafted(main), Error::Mismatch { .. }));
}

#[test]
fn a_derived_type_has_the_hash_of_the_type_written_out() {
    let written = "type Written = { name: string | null, age: int | null };";
    let workspace = workspace(&format!("{PARTIAL} {written}"));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |name: &str| engine.hash(&support::ty("main", name)).unwrap();

    assert_eq!(hash("Draft"), hash("Written"));
}

#[test]
fn a_type_cannot_be_derived_from_itself() {
    let main = "
        type Loop = comptime partial(Loop);
        fn main(): Loop => null;
    ";

    assert!(matches!(undrafted(main), Error::ComptimeCycle(_)));
}

#[test]
fn a_derived_type_refers_to_itself_by_a_reference() {
    // The call does not name the type, so it is no cycle: the tree
    // does, as that of a type written to refer to itself would.
    let source = "
        import ambient.tree;
        fn chain(): tree.Type => { union: [
            tree.nil,
            { record: [{ name: \"next\", holds: {
                named: { package: \"app\", module: [\"main\"], name: \"Chain\", kind: \"type\" },
            } }] },
        ] };
        type Chain = comptime chain();
        type Written = null | { next: Written };
        fn length(of: Chain): int {
            match of {
                null => { return 0; }
                let link => { return 1 + length(link.next); }
            }
        }
        fn main(): int => length({ next: { next: null } });
    ";

    assert_eq!(int(source), 2);
}

#[test]
fn a_derived_type_follows_the_type_it_was_derived_from() {
    let main = "fn main(): Draft => { name: null, age: null };";
    let wider = PARTIAL.replace("age: int", "age: int, email: string");
    let store = MemoryStore::new();
    let check = |types: &str| {
        let workspace = workspace(&format!("{types} {main}"));
        let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
        checked.map(|_| ())
    };

    assert_eq!(check(PARTIAL), Ok(()));
    assert!(matches!(check(&wider), Err(Error::Mismatch { .. })));
}

#[test]
fn a_derived_type_keeps_the_brands_and_names_it_was_handed() {
    let main = "
        brand Email;
        type Chain = { next: Chain | null, owner: string & Email };
        type Linked = comptime partial(Chain);
        type Written = { next: Chain | null, owner: string & Email | null };
    ";
    let workspace = workspace(&format!("{PARTIAL} {main}"));
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let hash = |name: &str| engine.hash(&support::ty("main", name)).unwrap();

    assert_eq!(hash("Linked"), hash("Written"));
}

#[test]
fn a_function_is_handed_a_reference_to_what_its_call_names() {
    let source = "
        import ambient.tree;
        fn double(n: int): int => n * 2;
        fn described(of: tree.Reference): string => \"${of.kind} ${of.name}\";
        fn main(): string => comptime described(double);
    ";

    assert_eq!(text(source), "value double");
}

#[test]
fn a_function_comes_by_a_reference_to_what_its_module_names() {
    let source = "
        import ambient.tree;
        import ambient.json;
        fn parser(): tree.Reference => comptime tree.reference(json.parse);
        fn name(of: tree.Reference): string => of.name;
        fn main(): string => name(parser());
    ";

    assert_eq!(text(source), "parse");
}

/// A function that gives code: a function that calls another twice.
const TWICE: &str = "
    import ambient.tree;
    fn double(n: int): int => n * 2;
    fn twice(of: tree.Reference): tree.Expr {
        let once: tree.Expr = { call: { definition: of }, given: [{ local: 0 }] };
        return {
            function: [tree.integer],
            body: { call: { definition: of }, given: [once] },
        };
    }
";

/// What is wrong with `main` of a program written after [`TWICE`].
fn unspliced(main: &str) -> Error {
    let workspace = workspace(&format!("{TWICE} {main}"));
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
    checked.unwrap_err()
}

#[test]
fn a_tree_of_code_is_spliced_as_the_code_it_is() {
    let main = "
        fn main(): int {
            let quadruple = comptime twice(double);
            return quadruple(5);
        }
    ";

    assert_eq!(int(&format!("{TWICE} {main}")), 20);
}

#[test]
fn spliced_code_is_checked_as_code_written_there() {
    let main = "
        fn shout(text: string): string => text ++ \"!\";
        fn main(): string {
            let wrong = comptime twice(shout);
            return wrong(\"hey\");
        }
    ";

    // The tree says the function takes an integer, and `shout` does not.
    assert!(matches!(unspliced(main), Error::Mismatch { .. }));
}

#[test]
fn spliced_code_names_what_the_body_does_not() {
    let main = "
        import ambient.json;
        fn parser(): tree.Expr => { definition: comptime tree.reference(json.write) };
        fn main(): string {
            let write = comptime parser();
            return write(3);
        }
    ";

    assert_eq!(text(&format!("{TWICE} {main}")), "3");
}

#[test]
fn an_error_in_spliced_code_is_at_the_call_and_shown_in_the_code() {
    let main = "
        import ambient.tree;
        fn wrong(): tree.Expr => { operation: \"add\", on: [{ literal: 1 }, { literal: \"two\" }] };
        fn main(): int => 2 * comptime wrong();
    ";
    let workspace = workspace(main);
    let store = MemoryStore::new();
    let engine = Engine::new(&workspace, &store);
    let error = engine.check(&value("main", "main")).unwrap_err();

    // The product, and then the call, which is its second operand.
    assert_eq!(engine.blame(&error), Some(2));
    assert_eq!(
        engine.shown(&error).unwrap(),
        "    1 + \"two\"\n    ^^^^^^^^^\n"
    );
}

#[test]
fn spliced_code_names_only_what_could_be_named_there() {
    let main = "
        fn hidden(): tree.Expr => {
            definition: { package: \"ambient\", module: [\"map\"], name: \"locate\", kind: \"value\" },
        };
        fn main(): unknown => comptime hidden();
    ";

    assert!(matches!(unspliced(main), Error::Unnameable { .. }));
}

#[test]
fn spliced_code_follows_what_it_names() {
    let main = "
        fn main(): int {
            let quadruple = comptime twice(double);
            return quadruple(5);
        }
    ";
    let texts = TWICE.replace(
        "fn double(n: int): int => n * 2;",
        "fn double(n: int): string => \"no\";",
    );
    let store = MemoryStore::new();
    let check = |functions: &str| {
        let workspace = workspace(&format!("{functions} {main}"));
        let checked = Engine::new(&workspace, &store).check(&value("main", "main"));
        checked.map(|_| ())
    };

    assert_eq!(check(TWICE), Ok(()));
    assert!(matches!(check(&texts), Err(Error::Mismatch { .. })));
}

#[test]
fn sibling_functions_in_a_tree_keep_their_own_parameters() {
    let main = "
        fn pair(of: tree.Reference): tree.Expr {
            let apply: tree.Expr = {
                function: [tree.integer],
                body: { call: { definition: of }, given: [{ local: 0 }] },
            };
            return { items: [apply, apply] };
        }
        fn main(): int {
            let both = comptime pair(double);
            match (both.get(0), both.get(1)) {
                (let first: fn(int): int, let second: fn(int): int) => { return first(1) + second(2); }
                _ => { return 0; }
            }
        }
    ";

    assert_eq!(int(&format!("{TWICE} {main}")), 6);
}

/// Abilities, and a function that takes them.
const ASKS: &str = "
    import ambient.tree;
    pub ability ask(question: string): int;
    pub ability tell(answer: int): null;
    fn work(): int with ask, tell {
        tell!(ask!(\"one\") + ask!(\"two\"));
        return 0;
    }
";

#[test]
fn a_function_named_to_a_call_is_handed_as_its_type() {
    let main = "
        fn row(of: tree.Type): string {
            match of {
                let it: { function: List<tree.Type>, returns: tree.Type, takes: List<tree.Ability> } => {
                    return it.takes.map(fn(taken) => \"${taken.ability.name}/${taken.function.length()}\").join_with(\" \");
                }
                _ => { return \"not a function\"; }
            }
        }
        fn main(): string => comptime row(work);
    ";

    assert_eq!(text(&format!("{ASKS} {main}")), "ask/1 tell/1");
}

#[test]
fn a_tree_binds_handlers_and_performs() {
    // A provider for `ask` that answers with the length of the
    // question, and tells it: every node a handler is made of.
    let main = "
        fn count(text: string): int => text.length();
        fn answers(of: tree.Type, count: tree.Reference): tree.Expr {
            match of {
                let it: { function: List<tree.Type>, returns: tree.Type, takes: List<tree.Ability> } => {
                    match (it.takes.get(0), it.takes.get(1)) {
                        (let ask: tree.Ability, let tell: tree.Ability) => {
                            let counted: tree.Expr = { call: { definition: count }, given: [{ local: 0 }] };
                            let told: tree.Expr = { perform: tell.ability, given: [{ local: 1 }] };
                            let answer: tree.Expr = { bind: counted, body: { bind: told, body: { local: 1 } } };
                            return { fields: [{ ability: ask.ability, value: { function: ask.function, body: answer } }] };
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            return { literal: null };
        }
        fn main(): int {
            let quiet = { [tell](answer) => null };
            return provide quiet => provide comptime answers(work, count) => ask!(\"four\");
        }
    ";

    assert_eq!(int(&format!("{ASKS} {main}")), 4);
}

#[test]
fn builders_put_code_together_without_counting_locals() {
    // A function of two that holds their sum and calls `double` on it,
    // inside a function that ignores its own parameter.
    let main = "
        import ambient.code;
        fn add(a: int, b: int): int => a + b;
        fn summed(add: tree.Reference, double: tree.Reference): tree.Expr {
            let inner = code.function([tree.integer, tree.integer], fn(pair) {
                let sum = code.call(code.definition(add), pair);
                return code.bind(sum, fn(held) => code.call(code.definition(double), [held]));
            });
            return code.build(code.function([tree.text], fn(ignored) => inner));
        }
        fn main(): int {
            let make = comptime summed(add, double);
            return make(\"x\")(3, 4);
        }
    ";

    assert_eq!(int(&format!("{TWICE} {main}")), 14);
}

/// Whether `main` checks, over `store`, where it splices a reference
/// to `shown` of another package, which is written as `other`.
fn splices_from(other: &str, store: &MemoryStore) -> Result<(), Error> {
    let main = "
        import ambient.tree;
        fn shown(): tree.Expr => {
            definition: { package: \"other\", module: [], name: \"shown\", kind: \"value\" },
        };
        fn main(): int => (comptime shown())();
    ";
    let mut workspace = workspace(main);
    let library = ModuleId::library(&ModuleId::new("other", "").package);
    workspace.set(library, parse(other).unwrap()).unwrap();
    workspace.depend("app", "other", "other").unwrap();
    let checked = Engine::new(&workspace, store).check(&value("main", "main"));
    checked.map(|_| ())
}

#[test]
fn a_stored_check_does_not_outlive_what_its_code_could_name() {
    // The content of `shown` is the same; only who may name it is not.
    let store = MemoryStore::new();
    let public = splices_from("pub fn shown(): int => 1;", &store);
    let private = splices_from("fn shown(): int => 1;", &store);

    assert_eq!(public, Ok(()));
    assert!(
        matches!(private, Err(Error::Unnameable { .. })),
        "{private:?}"
    );
}
