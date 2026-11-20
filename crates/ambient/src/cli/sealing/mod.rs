//! Sealing a native bundle: making it one object that shares no name
//! with the rest of a link but the functions of its externs. See
//! "Native bundles" in `docs/spec/linking.md`.
//!
//! A bundle is whatever its author's compiler made, and an archive
//! from a language with a library of its own carries that library: so
//! does the launcher, and so may the next bundle. Joined as they are,
//! each defines the same names. Sealed, each keeps its own copy to
//! itself, and the linker still leaves out what nothing reaches.
//!
//! The machine's linker joins the bundle into one object. Where it can
//! also say which names stay shared (Apple's), that is all; elsewhere
//! the object's symbols are rewritten here (`elf`).

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use object::{Object, ObjectSymbol};

use super::linking::{BUILT, EXTERN};

mod elf;

/// What begins an archive, as opposed to one object.
const ARCHIVE: &[u8] = b"!<arch>\n";

/// Whether the rest of a link sees the name `name`: only what names
/// an extern's function, or says what layout the bundle was built for.
fn shared(name: &[u8]) -> bool {
    [EXTERN, BUILT]
        .iter()
        .any(|prefix| name.starts_with(prefix.as_bytes()))
}

/// What tells the machine's linker to join all of the bundle at
/// `bundle` into one object, and on Apple's which names stay shared.
fn joining(bundle: &Path, archive: bool) -> Vec<String> {
    let bundle = bundle.to_string_lossy();
    if cfg!(target_vendor = "apple") {
        let shared = format!("-Wl,-exported_symbol,_{EXTERN}*,-exported_symbol,_{BUILT}*");
        let whole = if archive {
            format!("-Wl,-force_load,{bundle}")
        } else {
            bundle.into_owned()
        };
        return vec![shared, whole];
    }
    // A group of sections is kept once among the objects that hold
    // it. Left as groups, a later link would keep the launcher's copy
    // and drop this one, whose names are by then the bundle's own.
    vec![
        "-Wl,--force-group-allocation".to_owned(),
        "-Wl,--whole-archive".to_owned(),
        bundle.into_owned(),
        "-Wl,--no-whole-archive".to_owned(),
    ]
}

/// The object `joined`, with every name it defines but the shared ones
/// made its own.
///
/// # Errors
///
/// Fails if it defines an entry point of the runtime, or is no object
/// file this can read.
fn own(joined: &[u8]) -> Result<Vec<u8>, String> {
    let unread = |error: object::Error| format!("it is no object file: {error}");
    let file = object::File::parse(joined).map_err(unread)?;
    // A name of C is written after an underscore on Apple's machines.
    let written = |name: &'_ str| -> String {
        let apple = file.format() == object::BinaryFormat::MachO;
        let name = name.strip_prefix('_').filter(|_| apple).unwrap_or(name);
        name.to_owned()
    };
    let defined = file.symbols().filter(ObjectSymbol::is_definition);
    let names: Vec<String> = defined
        .filter_map(|symbol| symbol.name().ok().map(written))
        .collect();
    let entry = ambient_native::ENTRIES
        .iter()
        .find(|entry| names.iter().any(|name| name == entry.name));
    if let Some(entry) = entry {
        return Err(format!(
            "it defines `{}`, which is the runtime's: a bundle is linked with the runtime of \
             its program, and brings none",
            entry.name
        ));
    }
    let mut sealed = joined.to_vec();
    if file.format() == object::BinaryFormat::Elf {
        elf::own(&mut sealed, shared)?;
    }
    Ok(sealed)
}

/// Seals the bundle at `bundle` into the object at `sealed`, with the
/// linker `linker`.
///
/// # Errors
///
/// Fails with what the linker said, or if the bundle brings a runtime
/// of its own.
pub(super) fn seal(linker: &OsStr, bundle: &Path, sealed: &Path) -> Result<(), String> {
    let about = |reason: String| format!("the native bundle `{}`: {reason}", bundle.display());
    let failed = |error: std::io::Error| about(error.to_string());
    let archive = std::fs::read(bundle).map_err(failed)?.starts_with(ARCHIVE);
    let ran = Command::new(linker)
        .args(["-r", "-nostdlib"])
        .args(joining(bundle, archive))
        .arg("-o")
        .arg(sealed)
        .output()
        .map_err(failed)?;
    if !ran.status.success() {
        let said = String::from_utf8_lossy(&ran.stderr);
        return Err(about(format!("it was not joined:\n{}", said.trim_end())));
    }
    let joined = std::fs::read(sealed).map_err(failed)?;
    std::fs::write(sealed, own(&joined).map_err(about)?).map_err(failed)
}
