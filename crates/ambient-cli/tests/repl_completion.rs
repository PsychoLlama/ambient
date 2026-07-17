//! Tab completion over a live REPL session.
//!
//! Drives the real completer pipeline (`repl::completer::completions_for_line`)
//! against snapshots taken from an in-process [`ReplSession`], the same way
//! the interactive loop refreshes them — covering what the unit tests in
//! `completer.rs` can't: session state (saved bindings with recorded types,
//! committed `use` imports) flowing into completions.

mod repl_harness;

use ambient_cli::repl::completer::completions_for_line;
use ambient_cli::repl::session::CompletionSnapshot;
use repl_harness::ReplTest;

/// Complete `line` at its end against `snapshot`, returning the replacement
/// start and the item labels.
fn complete(snapshot: &CompletionSnapshot, line: &str) -> (usize, Vec<String>) {
    let resolver = ambient_analysis::platform_prelude_resolver();
    let (start, items) = completions_for_line(snapshot, &resolver, line, line.len());
    (start, items.into_iter().map(|i| i.label).collect())
}

#[test]
fn saved_bindings_complete_as_typed_locals() {
    let test = ReplTest::new().type_line("counter = 41");
    let snapshot = test.completion_snapshot();

    // The binding is an entry parameter in the trial source, annotated with
    // its recorded type — so it completes as a local.
    let (start, labels) = complete(&snapshot, "cou");
    assert_eq!(start, 0);
    assert!(labels.contains(&"counter".to_string()), "{labels:?}");

    test.shutdown();
}

#[test]
fn binding_members_complete_after_dot() {
    let test = ReplTest::new().type_line("xs = [1, 2]");
    let snapshot = test.completion_snapshot();
    assert!(
        snapshot.prefix.contains("xs: List<Number>"),
        "binding should be annotated with its recorded type: {:?}",
        snapshot.prefix
    );

    // Mid-word member completion: `xs.con` parses, so the receiver type
    // comes off the live checked trial module.
    let (start, labels) = complete(&snapshot, "xs.con");
    assert_eq!(start, 3);
    assert!(labels.contains(&"contains".to_string()), "{labels:?}");

    // Dangling dot: the trial no longer parses; the shared pipeline's
    // completion-time healing answers instead.
    let (start, labels) = complete(&snapshot, "xs.");
    assert_eq!(start, 3);
    assert!(labels.contains(&"contains".to_string()), "{labels:?}");

    test.shutdown();
}

#[test]
fn use_imported_module_members_complete() {
    let test = ReplTest::new().type_line("use core::collections::list;");
    let snapshot = test.completion_snapshot();

    // The committed import is re-emitted into the trial source, so the
    // alias qualifies member completion through the registry.
    let (start, labels) = complete(&snapshot, "list::");
    assert_eq!(start, 6);
    assert!(labels.contains(&"List".to_string()), "{labels:?}");

    test.shutdown();
}

#[test]
fn core_paths_complete_without_session_state() {
    let test = ReplTest::new();
    let snapshot = test.completion_snapshot();

    let (start, labels) = complete(&snapshot, "core::opt");
    assert_eq!(start, 6);
    assert!(labels.contains(&"option".to_string()), "{labels:?}");

    test.shutdown();
}
