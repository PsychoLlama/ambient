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
const SOURCES: [(&str, &str); 15] = [
    ("base64", include_str!("base64.ab")),
    ("bool", include_str!("bool.ab")),
    ("bytes", include_str!("bytes.ab")),
    ("cell", include_str!("cell.ab")),
    ("cleanup", include_str!("cleanup.ab")),
    ("map", include_str!("map.ab")),
    ("errors", include_str!("errors.ab")),
    ("float", include_str!("float.ab")),
    ("int", include_str!("int.ab")),
    ("json", include_str!("json.ab")),
    ("iterator", include_str!("iterator.ab")),
    ("list", include_str!("list.ab")),
    ("string", include_str!("string.ab")),
    ("url", include_str!("url.ab")),
    ("test", include_str!("test.ab")),
];

/// The key a value holds what releases it under, which a using reads.
#[must_use]
pub fn dispose() -> crate::symbol::Symbol {
    ModuleId::new(FAULT_PACKAGE, "cleanup").key("dispose")
}

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
    // What is thrown is reached where `Error` is.
    let shared = ["throw", "fail", "catch"].map(|name| format!("pkg.errors.{name}"));
    let module = shared
        .iter()
        .fold(module, |module, path| module.export(path));
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
    let module = module()
        .build()
        .map_err(|error| WorkspaceError::Library(error.to_string()))?;
    workspace.set(id.clone(), module)?;
    for (path, source) in SOURCES {
        let module = crate::syntax::parse(source)
            .map_err(|error| WorkspaceError::Library(format!("{path}.ab:{error}")))?;
        workspace.set(ModuleId::new(FAULT_PACKAGE, path), module)?;
    }
    workspace.standard(vec![id]);
    Ok(())
}
