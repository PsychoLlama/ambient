//! Writes `constants.js` beside the host: what the runtime defines and
//! the host must agree with, from the runtime, so that neither is
//! restated by hand. A word crosses into JavaScript as a signed
//! `BigInt`.
//!
//! The file is in the crate's directory, where a page can load it, and
//! is never committed.

use std::fmt::Write;

use ambient_runtime::{FALSE, FORSAKEN, Fault, Kind, NULL, SUSPENDING, TRUE, UNWIND, WAITING};

fn main() -> std::io::Result<()> {
    println!("cargo::rerun-if-changed=build.rs");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/constants.js");
    // Writing what is there already would make every build a change.
    if std::fs::read_to_string(path).is_ok_and(|kept| kept == constants()) {
        return Ok(());
    }
    std::fs::write(path, constants())
}

fn constants() -> String {
    let mut text = String::from(
        "// What the runtime defines and the host must agree with. Written by\n\
         // the crate's `build.rs` whenever the crate is built: never edit it.\n",
    );
    let words = [
        (
            "Words the runtime gives a meaning of their own.",
            "UNWIND",
            UNWIND,
        ),
        ("", "NULL", NULL),
        ("", "FALSE", FALSE),
        ("", "TRUE", TRUE),
        (
            "The modes of a context, as `amb_context_mode` gives them.",
            "SUSPENDING",
            SUSPENDING,
        ),
        ("", "WAITING", WAITING),
        (
            "The fault native code that fails is.",
            "NATIVE",
            Fault::Native as u64,
        ),
        (
            "The index native code is called with to be told a task left a wait.",
            "FORSAKEN",
            FORSAKEN,
        ),
    ];
    for (says, name, word) in words {
        if !says.is_empty() {
            let _ = write!(text, "\n// {says}\n");
        }
        let _ = writeln!(text, "export const {name} = {}n;", word.cast_signed());
    }
    let kinds = [
        ("null", Kind::Null),
        ("bool", Kind::Bool),
        ("int", Kind::Int),
        ("float", Kind::Float),
        ("string", Kind::String),
        ("tuple", Kind::Tuple),
        ("list", Kind::List),
        ("record", Kind::Record),
        ("function", Kind::Function),
        ("cell", Kind::Cell),
        ("tag", Kind::Tag),
        ("iterator", Kind::Iterator),
        ("bytes", Kind::Bytes),
        ("handle", Kind::Handle),
    ];
    text.push_str(
        "\n// What kind of value a word is, as `amb_kind` gives it.\nexport const Kind = {\n",
    );
    for (name, kind) in kinds {
        let _ = writeln!(text, "  {name}: {}n,", kind as u64);
    }
    text.push_str("};\n");
    text
}
