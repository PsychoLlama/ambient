//! The `system` package: the abilities of a program on a machine, and
//! the command-line target that binds them. See "The system package"
//! in `spec/targets.md`.
//!
//! Its modules are source, embedded and read at install, as the
//! standard library's are. Its native side is [`crate::host`].

use ambient_runtime::FAULT_PACKAGE;

use crate::symbol::{ModuleId, Symbol};
use crate::workspace::{Workspace, WorkspaceError};

/// The package's name and its ID, which no other package may take.
pub const PACKAGE: &str = "system";

/// Each module's path and text; the library module's path is empty.
const SOURCES: [(&str, &str); 9] = [
    ("", include_str!("lib.ab")),
    ("clock", include_str!("clock.ab")),
    ("console", include_str!("console.ab")),
    ("env", include_str!("env.ab")),
    ("fs", include_str!("fs.ab")),
    ("http", include_str!("http.ab")),
    ("path", include_str!("path.ab")),
    ("process", include_str!("process.ab")),
    ("random", include_str!("random.ab")),
];

/// The target a program run from a command line runs under.
#[must_use]
pub fn target() -> Symbol {
    ModuleId::new(PACKAGE, "").value("cli")
}

/// Adds the package to a workspace that has the standard library.
///
/// # Errors
///
/// Fails if the workspace already has a module where the package goes.
pub fn install(workspace: &mut Workspace) -> Result<(), WorkspaceError> {
    for (path, source) in SOURCES {
        let name = if path.is_empty() { "lib" } else { path };
        let module = crate::syntax::parse(source)
            .map_err(|error| WorkspaceError::Library(format!("{name}.ab:{error}")))?;
        workspace.set(ModuleId::new(PACKAGE, path), module)?;
    }
    workspace.depend(PACKAGE, FAULT_PACKAGE, FAULT_PACKAGE)
}
