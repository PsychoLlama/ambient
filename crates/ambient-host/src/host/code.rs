//! What `system.code` binds that needs no compiler: loading what was
//! compiled. See "Code" in `docs/spec/targets.md`.
//!
//! Packing a function and compiling one need the compiler, which adds
//! them (`ambient::host`).

use super::{Failure, Kind, PACKAGE, bytes, error};
use crate::native::{Natives, Running};
use crate::parts::Parts;
use crate::value::Value;

/// What a handler was given, refused as something it cannot take.
#[must_use]
pub fn invalid(message: &dyn ToString) -> Failure {
    Failure {
        kind: Kind::Invalid,
        message: message.to_string(),
    }
}

/// Why loaded code is refused: it names native code, which only the
/// program that loads it may bind.
#[must_use]
pub fn names_native(parts: &Parts) -> Option<String> {
    let native = parts.natives.first()?;
    Some(format!(
        "code to load names native code (`{}:{}`); it may only perform abilities",
        native.package, native.path
    ))
}

/// Loads what `code.compile` gave ([`Parts::encode`]) into the program
/// that asked. What names an extern is refused, so loaded code reaches
/// native code only through the abilities it performs.
fn load(running: &mut dyn Running, natives: &Natives, compiled: &[u8]) -> Result<Value, Failure> {
    let backend = running.backend().map_err(|error| invalid(&error))?;
    let parts = Parts::decode(compiled, backend).map_err(|error| invalid(&error))?;
    if let Some(refused) = names_native(&parts) {
        return Err(invalid(&refused));
    }
    running
        .load(&parts, natives)
        .map_err(|error| invalid(&error))
}

/// `natives`, with loading. What is loaded is given no native code but
/// what brings answers, since it names no extern.
pub(super) fn offer(natives: Natives) -> Natives {
    let base = natives
        .waits()
        .map_or_else(Natives::new, |waits| Natives::new().answered_by(waits));
    natives.reaching(PACKAGE, "code.load", move |running, args| {
        let done = load(running, &base, bytes(args, 0)?);
        Ok(done.unwrap_or_else(|failure| error("code", &failure)))
    })
}

#[cfg(test)]
mod tests {
    use super::names_native;
    use crate::parts::{Native, Parts};

    fn parts(natives: Vec<Native>) -> Parts {
        Parts {
            parts: Vec::new(),
            entry: 0,
            natives,
            open: false,
        }
    }

    #[test]
    fn only_code_that_names_no_native_is_loaded() {
        let native = Native {
            package: "app".to_owned(),
            path: "main.doubling".to_owned(),
            row: Vec::new(),
        };
        assert_eq!(names_native(&parts(Vec::new())), None);
        let refused = names_native(&parts(vec![native])).unwrap_or_default();
        assert!(refused.contains("`app:main.doubling`"), "{refused}");
    }
}
