//! An all and a first over a list of functions: as many tasks as a
//! value says. See "Joining" in `spec/tasks.md`.

#![allow(clippy::unwrap_used)]

mod support;

use ambient::value::Value;
use support::scripted::Scripted;
use support::text::text;

/// What `main` gives in a world where time passes only while every
/// task sleeps, and how long it took.
fn timed(body: &str) -> (String, i64) {
    let source = format!(
        "import ambient.iterator;
        import ambient.tasks;
        import ambient.time;
        import system.clock;

        fn nap(length: int): int with clock.sleep {{
            clock.sleep!(time.milliseconds(length));
            return length;
        }}

        fn main(): string with clock.sleep {{ {body} }}"
    );
    let (outcome, after) = Scripted::default().program(&source);
    match &outcome {
        Ok(Value::String(text)) => (text.clone(), after.time),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn every_item_is_worked_on_at_once_and_the_results_keep_their_order() {
    let body = r#"
        let slept = tasks.map([50, 10, 30], fn(length: int): int => nap(length));
        return slept.map(fn(length: int): string => length.to_text()).join_with(" ");
    "#;
    // Fifty milliseconds went by, not ninety.
    assert_eq!(timed(body), ("50 10 30".to_owned(), 50));
}

#[test]
fn the_first_to_be_over_is_what_a_race_gives_and_the_rest_are_stopped() {
    let body = r"
        let naps = [50, 10, 30].map(fn(length: int): fn(): int => fn(): int => nap(length));
        return (tasks.race(naps) as int else 0).to_text();
    ";
    assert_eq!(timed(body), ("10".to_owned(), 10));
}

#[test]
fn no_work_is_done_at_once_and_has_no_first() {
    let none = "let none: List<fn(): int> = [];";
    let gathered = format!("{none} return tasks.gather(none).length().to_text();");
    let raced = format!("{none} return tasks.race(none) == null ? \"none\" : \"some\";");
    assert_eq!(timed(&gathered), ("0".to_owned(), 0));
    assert_eq!(timed(&raced), ("none".to_owned(), 0));
}

#[test]
fn one_task_that_fails_stops_the_rest_and_fails_the_whole() {
    let body = r#"
        let work = fn(length: int): int => length == 10 ? 1 / (length - 10) : nap(length);
        let outcome = try => tasks.map([50, 10], work);
        match outcome {
            let error: Error => { return error.message; }
            _ => { return "finished"; }
        }
    "#;
    // The task that slept was cancelled: no time went by.
    assert_eq!(
        timed(body),
        ("an integer was divided by zero".to_owned(), 0)
    );
}

#[test]
fn each_task_started_is_a_step_of_the_budget_it_is_started_in() {
    let spent = |steps: i64| {
        text(&format!(
            "import ambient.tasks;
            fn main(): string {{
                let work = [1, 2, 3, 4].map(fn(n: int): fn(): int => fn(): int => n);
                let outcome = budget(steps: {steps}, allocation: 1000) {{
                    return tasks.gather(work).length();
                }};
                return outcome == 4 ? \"afforded\" : \"ran out\";
            }}"
        ))
    };
    assert_eq!(
        (spent(100), spent(3)),
        ("afforded".into(), "ran out".into())
    );
}

#[test]
fn thousands_of_tasks_wait_at_once() {
    // Each is its own stack natively, and all of them stand while they
    // sleep.
    let body = r"
        let lengths = iterator.range(0, 4000).to_list();
        let slept = tasks.map(lengths, fn(length: int): int => nap(length % 7));
        return slept.length().to_text();
    ";
    assert_eq!(timed(body), ("4000".to_owned(), 6));
}

#[test]
fn tasks_started_one_after_another_take_each_others_places() {
    // Each round's tasks are over before the next begins, and the
    // next takes the places they had.
    let body = r"
        let rounds = iterator.range(0, 300).to_list();
        let slept = rounds.map(fn(round: int): int =>
            tasks.map([1, 2, 3], fn(length: int): int => nap(length)).length());
        return slept.length().to_text();
    ";
    assert_eq!(timed(body), ("300".to_owned(), 900));
}
