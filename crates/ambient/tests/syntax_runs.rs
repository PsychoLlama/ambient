//! Programs written as text, run on every backend. See `spec/syntax.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::value::Value;
use support::text::{int, run, text};

#[test]
fn operators_bind_as_arithmetic_does() {
    assert_eq!(int("fn main(): int => 1 + 2 * 3 - 8 / 4 % 3;"), 5);
    assert_eq!(int("fn main(): int => (1 + 2) * 3;"), 9);
}

#[test]
fn comparisons_and_logic_are_built_from_what_the_language_has() {
    let source = r"
        fn pick(a: int, b: int): int {
            if a > b && a != 3 || b >= 10 { return 1; }
            if !(a <= b) { return 2; }
            return 3;
        }
        fn main(): int => pick(5, 1) * 100 + pick(3, 1) * 10 + pick(1, 2);
    ";

    assert_eq!(int(source), 123);
}

#[test]
fn logic_evaluates_only_what_it_needs() {
    let source = r"
        fn no(): bool => false;
        fn main(): bool => no() && 1 / 0 == 1 || !no();
    ";

    assert_eq!(run(source), Ok(Value::Bool(true)));
}

#[test]
fn statements_run_in_order_and_a_let_scopes_over_the_rest() {
    let source = r#"
        fn main(): string {
            let greeting = "hello";
            let (name, count) = ("world", 2);
            let { mark } = { mark: "!" };
            return greeting ++ " " ++ name ++ mark ++ count.to_text();
        }
    "#;

    assert_eq!(text(source), "hello world!2");
}

#[test]
fn what_follows_a_branch_that_returns_is_what_the_others_go_on_to() {
    let source = r"
        fn classify(n: int): int {
            if n < 0 {
                return 0 - 1;
            } else if n == 0 {
                let zero = 0;
                if zero == n { return 0; }
            }
            let doubled = n * 2;
            match doubled {
                4 => { return 40; }
                _ => {}
            }
            return doubled;
        }
        fn main(): int => classify(0 - 5) + classify(0) + classify(2) + classify(3);
    ";

    assert_eq!(int(source), -1 + 40 + 6);
}

#[test]
fn a_match_narrows_and_binds() {
    let source = r#"
        tag small;
        tag large;
        type Size = small | large;

        fn size(n: int): Size => match n < 10 { true => small, false => large };

        fn show(value: Size | int | (int, string)): string => match value {
            small => "small",
            large => "large",
            let n: int => n.to_text(),
            (let n, let unit) => n.to_text() ++ unit,
        };

        fn main(): string => show(size(3)) ++ show(size(30)) ++ show(7) ++ show((2, "kg"));
    "#;

    assert_eq!(text(source), "smalllarge72kg");
}

#[test]
fn a_record_holds_fields_functions_and_methods() {
    let source = r"
        type Counter = {
            count: int,
            next(self): Counter,
            twice(self): int,
        };

        fn counter(count: int): Counter {
            return {
                count,
                next(self): Counter => counter(self.count + 1),
                twice(self): int => self.count * 2,
            };
        }

        fn main(): int {
            let stepped = counter(1).next().next();
            let scaled = { ...stepped, count: 10 };
            return stepped.twice() + scaled.twice();
        }
    ";

    assert_eq!(int(source), 6 + 20);
}

#[test]
fn an_ability_is_performed_with_a_mark_and_provided_by_a_record() {
    let source = r"
        ability ask(): int;
        ability scale(by: int): int;

        fn twice(): int with ask, scale {
            return scale!(ask!() + ask!());
        }

        fn main(): int {
            let handlers = {
                [ask]() => 4,
                [scale](by) => by * 10,
            };
            return provide handlers => twice();
        }
    ";

    assert_eq!(int(source), 80);
}

#[test]
fn a_return_in_a_scope_gives_the_scope_its_value() {
    let source = r"
        ability stop(code: int): never;

        fn main(): int {
            let caught = provide { [stop](code) => code } {
                let reached = 1;
                stop!(reached + 6);
                return 0;
            };
            let bounded = try {
                return caught * 2;
            };
            return bounded as int else 0;
        }
    ";

    assert_eq!(int(source), 14);
}

#[test]
fn tasks_are_functions_and_cells_are_shared() {
    let source = r"
        fn main(): int {
            let total = cell<int>(1);
            let (a, b) = all(fn() => 2, fn() { return 3; });
            group {
                spawn { total.replace(total.read(), a + b); }
            }
            return total.read();
        }
    ";

    assert_eq!(int(source), 5);
}

#[test]
fn a_cast_narrows_unknown_or_gives_what_it_is_otherwise() {
    let source = r#"
        type Item = { name: string, qty: int };

        fn parse(value: unknown): Item => value as Item else { name: "?", qty: 0 };

        fn main(): int => parse({ name: "tea", qty: 2 }).qty + parse(7).qty;
    "#;

    assert_eq!(int(source), 2);
}

#[test]
fn generics_are_inferred_or_written_out() {
    let source = r"
        fn keep<T>(value: T): T => value;

        fn sum(items: List<int>): int => items.iter().fold(0, fn(total, item) => total + item);

        fn main(): int {
            let kept = keep::<int>;
            return kept(1) + keep(2) + sum([3, 4]);
        }
        fn a(): int { let kept = keep::<int>; return kept(1); }
        fn b(): int => keep(2);
        fn c(): int => sum([3, 4]);
    ";

    assert_eq!(int(source), 10);
}

#[test]
fn a_key_names_what_a_record_holds_without_a_field() {
    let source = r#"
        key display(): string;
        type Display = { [display]: fn(): string };

        fn show(value: Display): string => value[display]();

        fn main(): string {
            let point = { x: 1, [display](self) => "x=" ++ self.x.to_text() };
            return show(point);
        }
    "#;

    assert_eq!(text(source), "x=1");
}

#[test]
fn a_using_releases_what_it_binds_when_its_block_ends() {
    let source = r#"
        import ambient.cleanup.{dispose};

        fn noting(log: Cell<string>, name: string): { name: string, [dispose](): null } {
            return { name, [dispose]() { log.update(fn(text) => text ++ name); } };
        }
        fn main(): string {
            let log = cell<string>("");
            let seen = try {
                using first = noting(log, "a");
                using { name } = noting(log, "b");
                return first.name ++ name ++ log.read();
            };
            return (seen as string else "?") ++ ":" ++ log.read();
        }
    "#;

    // Each is released after the block's value is made, the last first.
    assert_eq!(text(source), "ab:ba");
}

#[test]
fn a_word_that_only_begins_a_definition_names_anything() {
    let source = r"
        tag tag;
        fn main(): int {
            let key = 1;
            let target = { module: 2, ability: key };
            return target.module + target.ability;
        }
    ";

    assert_eq!(int(source), 3);
}

#[test]
fn a_minus_before_a_value_asks_it_for_its_negative() {
    let source = r"
        fn main(): int {
            let (n, x) = (3, 1.5);
            return -n * 10 + ((-x * 2.0).to_int() as int else 0);
        }
    ";

    assert_eq!(int(source), -33);
}

#[test]
fn a_value_written_into_a_string_is_asked_for_its_text() {
    let source = r#"
        fn main(): string {
            let (name, count) = ("tea", 2);
            return "${count} of ${name}: ${"${count * 450}".length()} digits, \${no} $ {no}";
        }
    "#;

    assert_eq!(text(source), "2 of tea: 3 digits, ${no} $ {no}");
}

#[test]
fn a_module_in_a_row_is_every_ability_of_it() {
    let source = r#"
        import system.console;

        fn speak(): null with console {
            console.print!("a");
            console.warn!("b");
        }
        fn main(): string {
            let said = cell<string>("");
            let note = fn(text: string) { said.update(fn(so_far) => so_far ++ text); };
            let quiet = {
                [console.print](text) { note(text); },
                [console.warn](text) { note(text); },
                [console.write](text) { note(text); },
                [console.read_line]() => null,
                [console.read](most) => null,
            };
            provide quiet { speak(); }
            return said.read();
        }
    "#;

    assert_eq!(text(source), "ab");
}

#[test]
fn a_let_with_an_else_goes_on_only_where_the_value_fits() {
    let source = r#"
        fn half(n: int): int | string {
            if n % 2 == 1 { return "odd"; }
            return n / 2;
        }
        fn quarter(n: int): string {
            let once: int = half(n) else {
                let why => { return "${n} is ${why}"; }
            };
            let (twice: int, _) = (half(once), 0) else {
                _ => { return "${once} is odd"; }
            };
            return twice.to_text();
        }
        fn main(): string => "${quarter(12)}, ${quarter(6)}, ${quarter(5)}";
    "#;

    assert_eq!(text(source), "3, 3 is odd, 5 is odd");
}

#[test]
fn a_condition_chooses_between_two_values() {
    let source = r#"
        fn size(n: int): string => n > 10 ? "big" : n > 5 ? "middling" : "small";
        fn main(): string => "${size(20)} ${size(7)} ${size(1)}";
    "#;

    assert_eq!(text(source), "big middling small");
}

#[test]
fn a_parameter_of_a_lambda_is_taken_apart_by_a_pattern() {
    let source = r#"
        fn main(): string {
            let pairs = [("a", 1), ("b", 2)];
            let named = pairs.map(fn((name, count)) => "${name}${count}");
            let total = pairs.fold(0, fn(sum: int, (_, count): (string, int)): int => sum + count);
            return "${named.join_with(",")} ${total}";
        }
    "#;

    assert_eq!(text(source), "a1,b2 3");
}

#[test]
fn a_function_in_a_block_may_call_itself() {
    let source = r"
        fn sum(items: List<int>): int {
            fn go(at: int, total: int): int {
                let item: int = items.get(at) else {
                    null => { return total; }
                };
                return go(at + 1, total + item);
            }
            return go(0, 0);
        }
        fn main(): int {
            fn double(n: int): int => n * 2;
            return double(sum([1, 2, 3]));
        }
    ";

    assert_eq!(int(source), 12);
}

#[test]
fn a_let_that_states_a_type_holds_its_value_to_it_and_tests_nothing() {
    // A function type cannot be tested for, and each value takes from
    // the type what it leaves out: a parameter's type, a member's.
    let source = "
        fn main(): int {
            let add: fn(int): int = fn(n) => n + 1;
            let none: List<int> = [];
            let wide: int | null = 2;
            return add(none.length()) + (wide as int else 0);
        }
    ";

    assert_eq!(int(source), 3);
}

#[test]
fn a_for_runs_its_block_for_every_item() {
    let source = "
        import ambient.iterator.range;

        fn main(): int {
            let total = cell<int>(0);
            for n in range(0, 4) {
                total.update(fn(have) => have + n);
            }
            for (at, word) in [\"a\", \"bc\"].iter().enumerate() {
                if at > 0 {
                    total.update(fn(have) => have + word.length() * 100);
                }
            }
            return total.read();
        }
    ";

    assert_eq!(int(source), 206);
}

#[test]
fn a_number_with_an_exponent_is_a_float() {
    // After a dot it is no exponent: `2.e1` asks an integer for `e1`.
    let source = "fn main(): string => (1e3 + 2.5E-1 + 1_0e+1).to_text();";

    assert_eq!(text(source), "1100.25");
}

#[test]
fn a_let_that_states_a_type_holds_that_type_and_no_less() {
    // What the call gives is a string, but the binder was said to hold
    // anything, so the match must cover anything.
    let source = r#"
        fn main(): bool {
            let held: unknown = "ab".trim();
            return match held { let text: string => text.length() == 2, _ => false };
        }
    "#;
    assert_eq!(support::text::run(source), Ok(Value::Bool(true)));
}
