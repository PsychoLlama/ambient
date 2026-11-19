//! Brands: a mark on a type that only its module puts there. See
//! "Brands" in `spec/types.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use ambient::engine::Engine;
use ambient::error::Error;
use ambient::store::MemoryStore;
use support::scripted::check;
use support::text::{int, text, workspace};
use support::value;

/// What is wrong with a function of a program written as one module,
/// if anything is.
fn wrong(source: &str, function: &str) -> Option<Error> {
    let workspace = workspace(source);
    let store = MemoryStore::new();
    let checked = Engine::new(&workspace, &store).check(&value("main", function));
    checked.err()
}

/// What is wrong with `main` of a program written as one module.
fn refused(source: &str) -> Error {
    wrong(source, "main").unwrap()
}

const DURATION: &str = "
    brand Duration;
    fn seconds(count: int): int & Duration => Duration(count * 1000);
    fn wait(length: int & Duration): int => length / 1000;
";

#[test]
fn a_branded_value_is_used_as_its_type() {
    let source = format!(
        "{DURATION}
        fn main(): int {{
            let long = seconds(90);
            let plain: int = long;
            return wait(long) + long + plain.to_text().length();
        }}"
    );
    assert_eq!(int(&source), 90 + 90_000 + 5);
}

#[test]
fn a_value_without_the_brand_is_not_one_with_it() {
    let source = format!("{DURATION} fn main(): int => wait(90);");
    assert!(matches!(refused(&source), Error::Mismatch { .. }));

    // What an operation gives is not branded, whatever it was given.
    let summed = format!("{DURATION} fn main(): int => wait(seconds(1) + seconds(2));");
    assert!(matches!(refused(&summed), Error::Mismatch { .. }));
}

#[test]
fn brands_are_told_apart_and_gather() {
    let source = "
        brand Checked;
        brand Trimmed;
        fn checked(text: string): string & Checked => Checked(text);
        fn trimmed(text: string & Checked): string & Checked & Trimmed => Trimmed(text.trim());
        fn kept(text: string & Trimmed & Checked): string => text;
        fn main(): string => kept(trimmed(checked(\" ok \")));
    ";
    // Trimming gave a plain string, so the brand it had is gone.
    assert!(matches!(
        wrong(source, "trimmed"),
        Some(Error::Mismatch { .. })
    ));

    let held = source.replace("Trimmed(text.trim())", "Trimmed(text)");
    assert_eq!(text(&held), " ok ");
}

#[test]
fn a_literal_is_branded_as_its_primitive() {
    let source = "
        brand Port;
        fn port(): int & Port => Port(8080);
        fn main(): int => [port(), Port(80)].length() + port();
    ";
    assert_eq!(int(source), 8082);
}

#[test]
fn a_match_tests_what_is_under_a_brand() {
    let source = "
        brand Meters;
        fn shown(length: int & Meters | string): string {
            match length {
                let meters: int & Meters => { return \"${meters} m\"; }
                let said: string => { return said; }
            }
        }
        fn main(): string => shown(Meters(5)) + \", \" + shown(\"far\");
    ";
    assert_eq!(text(source), "5 m, far");
}

#[test]
fn a_match_cannot_brand_what_was_not() {
    // Every integer passes the test, and not every one is in meters.
    let source = "
        brand Meters;
        fn meters(length: int & Meters | int): int & Meters {
            match length {
                let meters: int & Meters => { return meters; }
                let other: int => { return Meters(0); }
            }
        }
        fn main(): int => meters(5);
    ";
    let cast = "
        brand Meters;
        fn main(): int => (cast::<int & Meters>(5) ?? Meters(0)) + 0;
    ";
    for (source, function) in [(source, "meters"), (cast, "main")] {
        let wrong = wrong(source, function);
        assert!(matches!(wrong, Some(Error::Untestable { .. })), "{wrong:?}");
    }
}

#[test]
fn only_a_brand_is_put_on_a_type() {
    let both = "fn main(): int & string => 1;";
    assert!(matches!(refused(both), Error::Intersection { .. }));
    let twice = "brand A; brand B; fn main(): A & B | null => null;";
    assert!(matches!(refused(twice), Error::Intersection { .. }));
}

#[test]
fn only_its_module_brands_a_value() {
    let file = |(path, text): (&str, &str)| (PathBuf::from(path), text.to_owned());
    let files: BTreeMap<PathBuf, String> = [
        ("app/ambient.toml", "name = \"app\"\nid = \"app\"\nversion = \"0.1.0\""),
        ("app/src/units.ab", "pub brand Meters;\npub fn meters(n: int): int & Meters => Meters(n);"),
        (
            "app/src/main.ab",
            "use pkg.units;\nuse pkg.units.{Meters};\nfn fine(): int & Meters => units.meters(1);\nfn main(): int => Meters(1);",
        ),
    ]
    .into_iter()
    .map(file)
    .collect();

    let (code, world) = check(&files);
    assert_eq!(code, 1);
    assert_eq!(
        world.err,
        "error: app/src/main.ab:4:19: app:main.main: only the module that declares `app:units.Meters` brands a value with it\n"
    );
}
