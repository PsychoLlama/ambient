//! The guide's examples: each block marked `ab` is a module that checks
//! and whose `main` runs.

#![allow(clippy::unwrap_used)]

mod support;

use support::scripted::{app, check, run};

/// The text of each block of `page` that is marked `ab`.
fn modules(page: &str) -> Vec<String> {
    let blocks = page.split("```").skip(1).step_by(2);
    let marked = blocks.filter_map(|block| block.strip_prefix("ab\n"));
    marked.map(str::to_owned).collect()
}

#[test]
fn every_module_in_the_guide_checks_and_runs() {
    let found = modules(include_str!("../../../docs/guide.md"));

    assert!(found.len() > 8);
    for module in found {
        let files = app(&module);
        // A `main` that gives an integer exits with it, so only what was
        // reported says whether running went wrong.
        let reported = [check(&files).1.err, run(&files).1.err];

        assert_eq!(reported, [String::new(), String::new()], "in:\n{module}");
    }
}
