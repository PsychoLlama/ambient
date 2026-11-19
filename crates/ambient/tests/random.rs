//! Random numbers: one ability, and what the `system` package makes of
//! it. See "The system package" in `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::{int, text};

/// `main` over bits that follow from a seed, with the module imported.
fn seeded(ret: &str, body: &str) -> String {
    format!(
        "use system.random;
        use ambient.iterator.{{range}};
        fn main(): {ret} => random.seeded(7, fn() {{ {body} }});"
    )
}

#[test]
fn the_same_seed_draws_the_same_numbers() {
    let draws = "return \"${random.between(0, 1000)} ${random.between(0, 1000)}\";";
    let first = text(&seeded("string", draws));

    assert_eq!(first, text(&seeded("string", draws)));
    assert_ne!(first, "0 0");
}

#[test]
fn a_number_between_two_is_never_outside_them() {
    let body = "
        return range(0, 200)
            .map(fn(_) => random.between(-3, 3))
            .filter(fn(drawn: int): bool => drawn < -3 || 2 < drawn)
            .count();";

    assert_eq!(int(&seeded("int", body)), 0);
}

#[test]
fn every_number_between_two_is_drawn() {
    let body = "
        let drawn = range(0, 200).map(fn(_) => random.between(0, 4)).to_list();
        return range(0, 4).filter(fn(wanted: int): bool => drawn.contains(wanted)).count();";

    assert_eq!(int(&seeded("int", body)), 4);
}

#[test]
fn a_fraction_is_less_than_one_and_not_less_than_zero() {
    let body = "
        return range(0, 200)
            .map(fn(_) => random.fraction())
            .filter(fn(drawn: float): bool => drawn < 0.0 || !(drawn < 1.0))
            .count();";

    assert_eq!(int(&seeded("int", body)), 0);
}

#[test]
fn a_shuffled_list_has_the_members_it_had() {
    let body = "
        let shuffled = random.shuffle([5, 3, 9, 1, 7, 2]);
        return shuffled.sort_by(fn(a: int, b: int): bool => a < b)
            .map(fn(n: int): string => n.to_text())
            .join_with(\" \");";

    assert_eq!(text(&seeded("string", body)), "1 2 3 5 7 9");
}

#[test]
fn a_member_is_picked_from_a_list_that_has_one() {
    let body = "
        let none: List<int> = [];
        let picked = random.pick([4, 4, 4]) ?? 0;
        return picked + (random.pick(none) ?? 100);";

    assert_eq!(int(&seeded("int", body)), 104);
}
