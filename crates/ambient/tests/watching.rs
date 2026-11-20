//! Watching files. See "Standard targets" in `docs/spec/targets.md`.

#![allow(clippy::unwrap_used)]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use ambient::host::{Change, Changed, System};
use ambient::value::Value;
use support::scripted::{Scripted, over};

fn text(text: &str) -> Value {
    Value::String(text.to_owned())
}

/// What a change is, written out.
const SEEN: &str = r#"
    use system.fs;
    use ambient.tasks;

    fn seen(change: fs.Change | null): string {
        match change {
            null => { return "closed"; }
            let change: fs.Change => {
                let how = match change.kind {
                    fs.created => "created",
                    fs.modified => "modified",
                    fs.removed => "removed",
                    fs.missed => "missed",
                };
                return "${how} ${change.path}";
            }
        }
    }
"#;

#[test]
fn a_watcher_gives_each_change_and_then_is_closed() {
    let source = r#"
        fn main(): string with fs {
            match fs.watch!("notes") {
                let error: Error => { return error.message; }
                let watcher: fs.Watcher => {
                    let first = seen(fs.change!(watcher));
                    let second = seen(fs.change!(watcher));
                    return "${first}, ${second}, ${seen(fs.change!(watcher))}";
                }
            }
        }
    "#;
    let change = |path: &str, kind| Change {
        path: path.to_owned(),
        kind,
    };
    let world = Scripted {
        changes: [
            change("notes/a", Changed::Created),
            change("notes", Changed::Missed),
        ]
        .into(),
        ..Scripted::default()
    };

    let (outcome, _) = world.program(&format!("{SEEN}{source}"));

    assert_eq!(outcome, Ok(text("created notes/a, missed notes, closed")));
}

#[test]
fn a_real_directory_is_watched_for_what_is_made_in_it() {
    let directory = std::env::temp_dir().join(format!("ambient-watched-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    let (watched, made) = (directory.to_str().unwrap(), directory.join("made"));
    let made = made.to_str().unwrap();
    // The watcher is there before the file is written, and whatever
    // waits on it when it is stopped is given nothing. Each backend
    // runs it, so what the last one made is taken away first.
    let source = format!(
        r#"{SEEN}
        fn main(): string with fs {{
            let _ = fs.remove!("{made}");
            match fs.watch!("{watched}") {{
                let error: Error => {{ return error.message; }}
                let watcher: fs.Watcher => {{
                    let (first, _) = tasks.all(
                        fn() => seen(fs.change!(watcher)),
                        fn() => fs.write!("{made}", "now"),
                    );
                    let (after, _) = tasks.all(
                        fn() => stopped(watcher),
                        fn() => fs.unwatch!(watcher),
                    );
                    return "${{first}}, ${{after}}";
                }}
            }}
        }}

        /// What a watcher gives once nothing more was written.
        fn stopped(watcher: fs.Watcher): string with fs.change {{
            match fs.change!(watcher) {{
                null => {{ return "closed"; }}
                let _: fs.Change => {{ return stopped(watcher); }}
            }}
        }}
    "#
    );

    let outcome = over(&source, || Rc::new(RefCell::new(System::new(Vec::new()))));
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(outcome, Ok(text(&format!("created {made}, closed"))));
}
