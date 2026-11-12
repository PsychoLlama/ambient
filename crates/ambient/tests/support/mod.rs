//! Shared scaffolding for engine tests: a workspace in memory, an engine
//! over it, and a backend that records what it was asked to compile.

#![allow(dead_code, clippy::unwrap_used)]

use std::cell::RefCell;

use ambient::builder::ModuleBuilder;
use ambient::core::Open;
use ambient::engine::Backend;
use ambient::symbol::{ModuleId, Symbol};
use ambient::workspace::Workspace;

/// A workspace from `(package, module path, module)` triples.
pub fn workspace<const N: usize>(modules: [(&str, &str, ModuleBuilder); N]) -> Workspace {
    let mut workspace = Workspace::new();
    for (package, path, module) in modules {
        workspace
            .set(ModuleId::new(package, path), module.build().unwrap())
            .unwrap();
    }
    workspace
}

/// The function `name` in module `path` of package `app`.
pub fn value(path: &str, name: &str) -> Symbol {
    ModuleId::new("app", path).value(name)
}

/// The type `name` in module `path` of package `app`.
pub fn ty(path: &str, name: &str) -> Symbol {
    ModuleId::new("app", path).ty(name)
}

/// A backend whose artifact is the tree's encoding.
#[derive(Default)]
pub struct Recording {
    pub compiled: RefCell<usize>,
}

impl Backend for Recording {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn compile(&self, definition: &Open) -> Result<Vec<u8>, String> {
        *self.compiled.borrow_mut() += 1;
        Ok(definition.tree.encode())
    }
}
