//! The standard library: the language's own package, written with
//! builders like any other. See `spec/modules.md` and `spec/errors.md`.
//!
//! The kind of an error a boundary makes, and the limit a budget ran out
//! of, are tags of this package. A tag written in a program is one of
//! the program's own package, so a program names these through the types
//! defined here, which a workspace has in scope unless it lists preludes
//! of its own.

mod shapes;

use ambient_runtime::{FAULT_PACKAGE, Fault, LIMITS};

use crate::builder::{self as b, ModuleBuilder, ty};
use crate::symbol::ModuleId;
use crate::workspace::{Workspace, WorkspaceError};

/// The name of a tag's type: `no_match` is `NoMatch`.
fn type_name(tag: &str) -> String {
    let capitalized = |word: &str| {
        let mut letters = word.chars();
        let first = letters.next().map(|letter| letter.to_ascii_uppercase());
        first.into_iter().chain(letters).collect::<String>()
    };
    tag.split('_').map(capitalized).collect()
}

/// The library module: `Error`, and a type for the kind of each fault
/// and for each limit of a budget. It lists the module of each shape.
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
    tags.fold(module, |module, tag| {
        module.define(b::type_(&type_name(tag), ty::tagged(tag)).public())
    })
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
