//! The standard library: the language's own package, written with
//! builders or as source like any other. See `spec/modules.md` and `spec/errors.md`.
//!
//! The kind of an error a boundary makes, and the limit a budget ran out
//! of, are tags this package declares in its library module, which a
//! workspace has in scope unless it lists preludes of its own.

use ambient_runtime::{FAULT_PACKAGE, Fault, LIMITS};

use crate::builder::{self as b, ModuleBuilder, ty};
use crate::symbol::ModuleId;
use crate::workspace::{Workspace, WorkspaceError};

/// The modules written as source: the path of each, and its text. One
/// named for a shape is that shape's methods.
const SOURCES: [(&str, &str); 13] = [
    ("cell", include_str!("cell.ab")),
    ("cleanup", include_str!("cleanup.ab")),
    ("float", include_str!("float.ab")),
    ("int", include_str!("int.ab")),
    ("iterator", include_str!("iterator.ab")),
    ("list", include_str!("list.ab")),
    ("string", include_str!("string.ab")),
    ("clock", include_str!("clock.ab")),
    ("console", include_str!("console.ab")),
    ("env", include_str!("env.ab")),
    ("fs", include_str!("fs.ab")),
    ("process", include_str!("process.ab")),
    ("system", include_str!("system.ab")),
];

/// The library module: `Error`, and a tag for the kind of each fault
/// and for each limit of a budget, named as the runtime names it. It
/// lists the module of each shape.
#[must_use]
pub fn module() -> ModuleBuilder {
    let frame = ty::record([("definition", ty::string()), ("position", ty::int())]);
    let error = ty::record([
        ("kind", ty::tag()),
        ("message", ty::string()),
        ("cause", ty::union([ty::named("Error"), ty::null()])),
        ("trace", ty::list(ty::named("Frame"))),
    ]);
    let kinds = Fault::ALL.iter().filter_map(|fault| fault.describe());
    let tags = kinds.map(|(kind, _)| kind).chain(LIMITS);
    let paths = SOURCES.map(|(path, _)| path).into_iter();
    let module = paths.fold(b::module(), ModuleBuilder::expose);
    let module = module
        .define(b::type_("Frame", frame).public())
        .define(b::type_("Error", error).public());
    tags.fold(module, |module, tag| module.define(b::tag(tag).public()))
}

/// Adds the standard library to a workspace, as the prelude of every
/// package that lists none and whose workspace lists none.
///
/// # Errors
///
/// Fails if the workspace already has a module where the library goes.
pub fn install(workspace: &mut Workspace) -> Result<(), WorkspaceError> {
    let id = ModuleId::new(FAULT_PACKAGE, "");
    let module = module().build().map_err(|_| WorkspaceError::Library)?;
    workspace.set(id.clone(), module)?;
    for (path, source) in SOURCES {
        let module = crate::syntax::parse(source).map_err(|_| WorkspaceError::Library)?;
        workspace.set(ModuleId::new(FAULT_PACKAGE, path), module)?;
    }
    workspace.standard(vec![id]);
    Ok(())
}
