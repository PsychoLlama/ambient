//! The standard library: the language's own package, written with
//! builders like any other. See `spec/modules.md` and `spec/errors.md`.
//!
//! The kind of an error a boundary makes, and the limit a budget ran out
//! of, are tags this package declares in its library module, which a
//! workspace has in scope unless it lists preludes of its own.

mod iterator;
mod shapes;

use ambient_runtime::{FAULT_PACKAGE, Fault, LIMITS};

use crate::builder::{self as b, ModuleBuilder, ty};
use crate::symbol::ModuleId;
use crate::workspace::{Workspace, WorkspaceError};

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
    let shapes = shapes::modules().into_iter();
    let module = shapes.fold(b::module(), |module, (shape, _)| {
        module.expose(shape.module())
    });
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
    for (shape, module) in shapes::modules() {
        let module = module.build().map_err(|_| WorkspaceError::Library)?;
        workspace.set(ModuleId::new(FAULT_PACKAGE, shape.module()), module)?;
    }
    workspace.standard(vec![id]);
    Ok(())
}
