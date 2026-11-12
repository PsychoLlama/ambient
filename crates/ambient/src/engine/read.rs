//! Read: a module's source yields its definition table and surface trees.
//! Then, per module, the scope its definitions are resolved in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Engine, Query, Stage, memo};
use crate::builder::OWN_PACKAGE;
use crate::error::Error;
use crate::hash::{Hash, digest};
use crate::surface::{Definition, DefinitionKind, Import, Module};
use crate::symbol::{ModuleId, ModulePath, Namespace, Symbol};

/// What a module defines, without any body.
///
/// Other modules resolve against this, so a body edit never reaches them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table(pub BTreeMap<(Namespace, String), Entry>);

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
    pub definitions: Vec<Definition>,
    /// A digest of each definition, names included.
    pub digests: Vec<Hash>,
}

/// Every name a module's definitions can use, besides their own locals.
#[derive(Debug, Default, Serialize)]
pub struct Scope {
    /// The module's own definitions and the definitions it imports.
    pub definitions: BTreeMap<(Namespace, String), Symbol>,
    /// The modules it imports, each with its table, by alias.
    pub modules: BTreeMap<String, (ModuleId, Table)>,
}

pub(super) struct Scoped {
    pub scope: Scope,
    pub digest: Hash,
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

    pub(super) fn scope(&self, id: &ModuleId) -> Query<Scoped> {
        memo(&self.scope, id, || {
            let own = self.read(id)?;
            let mut scope = Scope::default();
            for (namespace, name) in own.table.0.keys() {
                scope
                    .definitions
                    .insert((*namespace, name.clone()), id.symbol(*namespace, name));
            }
            for import in &own.imports {
                self.import(id, import, &mut scope)?;
            }
            let digest = digest("scope", &scope);
            Ok(std::sync::Arc::new(Scoped { scope, digest }))
        })
    }

    fn import(&self, id: &ModuleId, import: &Import, scope: &mut Scope) -> Result<(), Error> {
        let unknown = || Error::UnknownImport {
            module: id.clone(),
            path: import.path.clone(),
        };
        let duplicate = || Error::DuplicateName {
            module: id.clone(),
            name: import.alias.clone(),
        };

        let (first, rest) = import.path.0.split_first().ok_or_else(unknown)?;
        let package = if first == OWN_PACKAGE {
            id.package.clone()
        } else {
            self.sources
                .dependency(&id.package, first)
                .ok_or_else(|| Error::UnknownPackage {
                    module: id.clone(),
                    name: first.clone(),
                })?
        };

        let taken = scope.modules.contains_key(&import.alias)
            || scope
                .definitions
                .keys()
                .any(|(_, name)| *name == import.alias);

        // A segment names a file or a directory, never both, so at most one
        // of these two readings finds anything.
        let whole = ModuleId {
            package: package.clone(),
            path: ModulePath(rest.to_vec()),
        };
        if self.sources.module(&whole).is_some() {
            if taken {
                return Err(duplicate());
            }
            let table = self.read(&whole)?.table.clone();
            scope.modules.insert(import.alias.clone(), (whole, table));
            return Ok(());
        }

        let (name, parent) = rest.split_last().ok_or_else(unknown)?;
        let parent = ModuleId {
            package,
            path: ModulePath(parent.to_vec()),
        };
        if self.sources.module(&parent).is_none() {
            return Err(unknown());
        }
        let table = &self.read(&parent)?.table;
        let mut found = false;
        for namespace in [Namespace::Type, Namespace::Value, Namespace::Ability] {
            let Some(entry) = table.0.get(&(namespace, name.clone())) else {
                continue;
            };
            if !entry.public {
                return Err(Error::PrivateImport {
                    module: id.clone(),
                    path: import.path.clone(),
                });
            }
            if taken {
                return Err(duplicate());
            }
            found = true;
            scope.definitions.insert(
                (namespace, import.alias.clone()),
                parent.symbol(namespace, name),
            );
        }
        if found { Ok(()) } else { Err(unknown()) }
    }
}

fn read(module: &Module) -> Read {
    let mut table = Table::default();
    let mut digests = Vec::new();
    for (index, definition) in module.definitions().iter().enumerate() {
        let namespace = match definition.kind {
            DefinitionKind::Type(_) => Namespace::Type,
            DefinitionKind::Function(_) => Namespace::Value,
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
        definitions: module.definitions().to_vec(),
        digests,
    }
}
