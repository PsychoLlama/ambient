//! Read: a module's source yields its definition table and surface trees.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Engine, Query, Stage, memo};
use crate::error::Error;
use crate::hash::{Hash, digest};
use crate::surface::{Definition, DefinitionKind, Import, Module};
use crate::symbol::{ModuleId, ModulePath, Namespace};

/// What a module defines, without any body.
///
/// Other modules resolve against this, so a body edit never reaches them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table(pub BTreeMap<(Namespace, String), Entry>);

impl Table {
    /// Whether the module defines `name`, in any namespace.
    #[must_use]
    pub fn names(&self, name: &str) -> bool {
        self.0.keys().any(|(_, defined)| defined == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Position in [`Read::definitions`].
    pub index: usize,
    pub public: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Read {
    pub table: Table,
    pub imports: Vec<Import>,
    /// The modules a library module lets other packages reach.
    pub exposes: Vec<ModulePath>,
    pub definitions: Vec<Definition>,
    /// A digest of each definition, names included.
    pub digests: Vec<Hash>,
}

impl Engine<'_> {
    /// A module's definition table and surface trees.
    ///
    /// # Errors
    ///
    /// Fails if there is no such module.
    pub fn read(&self, id: &ModuleId) -> Query<Read> {
        memo(&self.read, id, || {
            let module = self
                .sources
                .module(id)
                .ok_or_else(|| Error::UnknownModule(id.clone()))?;
            let key = digest("read", &*module);
            self.stage(Stage::Read, id, key, || Ok(read(&module)))
        })
    }
}

fn read(module: &Module) -> Read {
    let mut table = Table::default();
    let mut digests = Vec::new();
    for (index, definition) in module.definitions().iter().enumerate() {
        let namespace = match definition.kind {
            DefinitionKind::Type(_) => Namespace::Type,
            DefinitionKind::Function(_) | DefinitionKind::Extern(_) | DefinitionKind::Target(_) => {
                Namespace::Value
            }
            DefinitionKind::Ability(_) => Namespace::Ability,
        };
        table.0.insert(
            (namespace, definition.name.clone()),
            Entry {
                index,
                public: definition.public,
            },
        );
        digests.push(digest("definition", definition));
    }
    Read {
        table,
        imports: module.imports().to_vec(),
        exposes: module
            .exposes()
            .iter()
            .map(|path| ModulePath(path.0.clone()))
            .collect(),
        definitions: module.definitions().to_vec(),
        digests,
    }
}
