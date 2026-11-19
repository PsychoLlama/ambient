//! Paths: pure functions over the text of one. See "The system
//! package" in `spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::text;

/// What `expr` gives, with `system.path` imported.
fn path(expr: &str) -> String {
    text(&format!("use system.path;\nfn main(): string => {expr};"))
}

/// What a function of a path gives for each of `paths`, null as `-`.
fn each(function: &str, paths: [&str; 4]) -> String {
    let calls = paths.map(|path| format!("(cast::<string>(path.{function}(\"{path}\"), \"-\"))"));
    path(&format!("[{}].join_with(\" \")", calls.join(", ")))
}

#[test]
fn a_path_has_parts_between_its_separators() {
    assert_eq!(
        path("path.parts(\"/a//b/c.txt\").join_with(\",\")"),
        "a,b,c.txt"
    );
}

#[test]
fn paths_are_joined_each_inside_the_one_before() {
    let joined =
        "[path.join([\"a\", \"b/\", \"c.txt\"]), path.join([\"a\", \"/etc\", \"\", \"x\"])]";

    assert_eq!(
        path(&format!("{joined}.join_with(\" \")")),
        "a/b/c.txt /etc/x"
    );
}

#[test]
fn a_path_has_a_name_and_what_it_is_inside() {
    let paths = ["/a/b.txt", "b.txt", "/a", "/"];

    assert_eq!(each("name", paths), "b.txt b.txt a ");
    assert_eq!(each("parent", paths), "/a - / -");
}

#[test]
fn a_name_has_a_stem_and_an_extension() {
    let paths = ["/a/b.tar.gz", "notes", ".profile", "x/.config.toml"];

    assert_eq!(each("extension", paths), "gz - - toml");
    assert_eq!(each("stem", paths), "b.tar notes .profile .config");
}
