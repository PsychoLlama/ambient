//! Packing a function and loading one, as a program does. See "Code"
//! in `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::value::Value;
use support::scripted::Scripted;

/// What `main` gives, with `rest` beside it, in a world, and the world
/// it leaves.
fn run(rest: &str, world: &Scripted) -> (Value, Scripted) {
    let source = format!(
        "import system.code;
        import system.fs;

        {rest}"
    );
    let (outcome, after) = world.program(&source);
    (outcome.unwrap(), after)
}

fn text(text: &str) -> Value {
    Value::String(text.to_owned())
}

/// A function to send: it reaches another, and the library.
const SENT: &str = r#"
    fn loud(word: string): string => word.to_upper() ++ "!";
    fn greet(name: string): string => "hello, ${loud(name)}";
"#;

/// Loads what was packed as a function from a string to a string, and gives
/// what it makes of `ada`, or why it could not.
const CALLED: &str = r#"
    fn other(name: string): string => "not that function";

    fn called(packed: bytes): string with code.load {
        match code.load!(packed) {
            let error: Error => { return error.message; }
            let loaded: unknown => {
                let greet = loaded as fn(string): string else other;
                return greet("ada");
            }
        }
    }
"#;

#[test]
fn a_function_is_packed_and_loaded_where_it_was_never_written() {
    let sender = format!(
        "{SENT}
        fn main(): null with code.pack, fs.write_bytes {{
            let _ = fs.write_bytes!(\"greet.pack\", code.pack!(greet) as bytes else 0x[]);
            return null;
        }}"
    );
    let (_, sent) = run(&sender, &Scripted::default());

    let receiver = format!(
        "{CALLED}
        fn main(): string with code.load, fs.read_bytes =>
            called(fs.read_bytes!(\"greet.pack\") as bytes else 0x[]);"
    );
    let (gave, _) = run(&receiver, &sent);
    assert_eq!(gave, text("hello, ADA!"));
}

#[test]
fn a_program_loads_what_it_packed_itself() {
    let source = format!(
        "{SENT} {CALLED}
        fn main(): string with code => called(code.pack!(greet) as bytes else 0x[]);"
    );
    assert_eq!(run(&source, &Scripted::default()).0, text("hello, ADA!"));
}

#[test]
fn a_loaded_function_is_only_what_its_cast_says() {
    let source = format!(
        "{CALLED}
        fn count(name: string): int => name.length();
        fn main(): string with code => called(code.pack!(count) as bytes else 0x[]);"
    );
    assert_eq!(
        run(&source, &Scripted::default()).0,
        text("not that function")
    );
}

#[test]
fn bytes_that_are_not_code_are_an_error() {
    let source = format!(
        "{CALLED}
        fn main(): string with code => called(0x[01 02 03]);"
    );
    assert_eq!(run(&source, &Scripted::default()).0, text("not a pack"));
}

#[test]
fn only_a_function_that_captured_nothing_is_packed() {
    let source = r#"
        fn refused(function: unknown): string with code.pack {
            match code.pack!(function) {
                let error: Error => { return error.kind == code.invalid ? error.message : "other"; }
                let _: bytes => { return "packed"; }
            }
        }
        fn main(): string with code {
            let name = "ada";
            let captured = fn(): string => name;
            return refused(captured) ++ "; " ++ refused(7);
        }
    "#;
    let gave = run(source, &Scripted::default()).0;
    let expected = "a function that captured something is not packed; \
        what was given is not a function";
    assert_eq!(gave, text(expected));
}

#[test]
fn a_loaded_function_does_what_its_caller_provides() {
    let source = r#"
        import system.console;

        fn quiet(word: string): null with console.print => null;
        fn shout(word: string): null with console.print {
            console.print!(word.to_upper());
        }
        fn main(): null with code, console.print {
            let loaded = code.load!(code.pack!(shout) as bytes else 0x[]);
            let shout = loaded as fn(string): null with console.print else quiet;
            shout("loaded");
        }
    "#;
    let (_, after) = run(source, &Scripted::default());
    assert_eq!(after.out, "LOADED\n");
}

#[test]
fn a_program_goes_on_with_the_code_it_was_last_given() {
    // The pattern of `spec/upgrade.md`: what answers is read from a
    // cell each time, and a deploy replaces what the cell holds.
    let source = r#"
        type Answer = fn(string): string;

        fn first(name: string): string => "hello, ${name}";
        fn second(name: string): string => "goodbye, ${name}";

        fn deploy(current: Cell<Answer>, packed: bytes): null with code.load {
            let loaded = code.load!(packed);
            let _ = current.update(fn(now: Answer): Answer => loaded as Answer else now);
        }

        fn main(): string with code {
            let current = cell<Answer>(first);
            let before = current.read()("ada");
            deploy(current, 0x[00]);
            let kept = current.read()("ada");
            deploy(current, code.pack!(second) as bytes else 0x[]);
            return "${before}; ${kept}; ${current.read()("ada")}";
        }
    "#;
    let gave = run(source, &Scripted::default()).0;
    assert_eq!(gave, text("hello, ada; hello, ada; goodbye, ada"));
}
