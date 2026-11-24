//! The standard library's set, over its map. See "Sets" in
//! `docs/spec/maps.md`.

mod support;

use ambient::value::Value;
use proptest::prelude::*;
use support::text::{int, run, text};

/// A program over `s`, a set of 3, 1, and 2, and `t`, of 2, 3, and 4,
/// whose `main` gives `ret` from `body`, beside `written`, which writes
/// keys out.
fn over(ret: &str, body: &str) -> String {
    format!(
        r#"
        use ambient.set;
        fn written(keys: List<int>): string =>
            keys.map(fn(key: int): string => key.to_text()).join_with(",");
        fn main(): {ret} {{
            let s = set.of([3, 1, 2, 1]);
            let t = set.of([2, 3, 4]);
            {body}
        }}
        "#
    )
}

#[test]
fn a_set_holds_each_key_once_in_order() {
    assert_eq!(int(&over("int", "return s.size();")), 3);
    assert_eq!(text(&over("string", "return written(s.keys());")), "1,2,3");
}

#[test]
fn changing_a_set_leaves_the_one_changed_as_it_was() {
    let body = r#"
        let more = s.add(5).remove(1);
        return written(more.keys()) + " " + written(s.keys());
    "#;
    assert_eq!(text(&over("string", body)), "2,3,5 1,2,3");
}

#[test]
fn two_sets_combine() {
    let body = r#"
        let union = written(s.union(t).keys());
        let both = written(s.intersection(t).keys());
        return union + " " + both + " " + written(s.difference(t).keys());
    "#;
    assert_eq!(text(&over("string", body)), "1,2,3,4 2,3 1");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn a_set_has_what_was_put_in_it(
        keys in prop::collection::vec(0u8..32, 0..24),
        asked in 0u8..32,
    ) {
        let source = format!("use ambient.set; fn main(): bool => set.of::<int>({keys:?}).has({asked});");
        prop_assert_eq!(run(&source), Ok(Value::Bool(keys.contains(&asked))));
    }
}
