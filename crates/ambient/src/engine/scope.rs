//! Scope: every name a module's definitions can use, and every name
//! other modules can reach through it. See `spec/modules.md`.

use std::collections::BTreeMap;

use ambient_runtime::FAULT_PACKAGE;
use serde::Serialize;

use super::{Engine, Query, memo};
use crate::builder::OWN_PACKAGE;
use crate::error::Error;
use crate::hash::{Hash, digest};
use crate::surface::{Import, Shape};
use crate::symbol::{ModuleId, ModulePath, Namespace, Symbol};

/// Something a module lets be named through it: a definition of its own,
/// or one it re-exports.
#[derive(Clone, Debug, Serialize)]
pub struct Member {
    /// What the name means. A re-export is an alias: this is the symbol
    /// of the definition, wherever it is.
    pub symbol: Symbol,
    /// Whether other modules may use the name.
    pub public: bool,
}

/// A module's members, by name.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Members(pub BTreeMap<(Namespace, String), Member>);

/// Every name a module's definitions can use, besides their own locals.
#[derive(Debug, Default, Serialize)]
pub struct Scope {
    /// The module's own definitions and the definitions it imports.
    pub definitions: BTreeMap<(Namespace, String), Symbol>,
    /// The modules it imports, each with its members, by alias.
    pub modules: BTreeMap<String, (ModuleId, Members)>,
    /// What its package's preludes bring into scope. Anything above
    /// shadows these.
    pub preludes: BTreeMap<(Namespace, String), Symbol>,
    /// The methods of the built-in shapes, by name: the public
    /// functions of each shape's module of the standard library. No
    /// name in scope shadows one, since a method is not found by name
    /// alone. See `spec/methods.md`.
    pub methods: BTreeMap<String, Vec<(Shape, Symbol)>>,
}

impl Scope {
    /// What `name` means on its own: the module's definitions and imports
    /// first, then its package's preludes.
    #[must_use]
    pub fn find(&self, namespace: Namespace, name: &str) -> Option<&Symbol> {
        let key = (namespace, name.to_owned());
        self.definitions
            .get(&key)
            .or_else(|| self.preludes.get(&key))
    }
}

pub(super) struct Scoped {
    pub scope: Scope,
    pub digest: Hash,
}

impl Engine<'_> {
    pub(super) fn scope(&self, id: &ModuleId) -> Query<Scoped> {
        memo(&self.scope, id, || {
            let own = self.read(id)?;
            let mut scope = Scope::default();
            // A later prelude wins where two bring in one name.
            for prelude in self.sources.preludes(&id.package) {
                self.exposed(id, &prelude)?;
                for (key, member) in &self.members(&prelude)?.0 {
                    if member.public {
                        scope.preludes.insert(key.clone(), member.symbol.clone());
                    }
                }
            }
            for shape in Shape::ALL {
                let module = ModuleId::new(FAULT_PACKAGE, shape.module());
                if self.sources.module(&module).is_none() {
                    continue;
                }
                for ((namespace, name), member) in &self.members(&module)?.0 {
                    if member.public && *namespace == Namespace::Value {
                        let methods = scope.methods.entry(name.clone()).or_default();
                        methods.push((shape, member.symbol.clone()));
                    }
                }
            }
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

    /// What can be named through a module: its definitions, and the
    /// definitions it re-exports, which keep their own symbols.
    pub(super) fn members(&self, id: &ModuleId) -> Query<Members> {
        memo(&self.members, id, || {
            if self.exporting.borrow().contains(id) {
                return Err(Error::ExportCycle(id.clone()));
            }
            self.exporting.borrow_mut().push(id.clone());
            let members = self.members_of(id);
            self.exporting.borrow_mut().pop();
            members.map(std::sync::Arc::new)
        })
    }

    fn members_of(&self, id: &ModuleId) -> Result<Members, Error> {
        let own = self.read(id)?;
        let mut members = Members::default();
        for ((namespace, name), entry) in &own.table.0 {
            let symbol = id.symbol(*namespace, name);
            let public = entry.public;
            members
                .0
                .insert((*namespace, name.clone()), Member { symbol, public });
        }
        for import in own.imports.iter().filter(|import| import.public) {
            let mut found = Scope::default();
            self.import(id, import, &mut found)?;
            if !found.modules.is_empty() {
                return Err(Error::ExportsModule {
                    module: id.clone(),
                    path: import.path.clone(),
                });
            }
            for (key, symbol) in found.definitions {
                let public = true;
                members.0.entry(key).or_insert(Member { symbol, public });
            }
        }
        Ok(members)
    }

    /// Fails unless a module of `from` may reach `module`: it is of the
    /// same package, or it is the library module of its own, or that
    /// library module lists it.
    fn exposed(&self, from: &ModuleId, module: &ModuleId) -> Result<(), Error> {
        if module.package == from.package || module.path.0.is_empty() {
            return Ok(());
        }
        let library = ModuleId::library(&module.package);
        let listed = self.sources.module(&library).is_some()
            && self.read(&library)?.exposes.contains(&module.path);
        if listed {
            return Ok(());
        }
        Err(Error::InternalModule {
            module: from.clone(),
            path: crate::surface::Path(module.path.0.clone()),
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
        // An error about reach names the import as it was written.
        let reach = |module: &ModuleId| {
            self.exposed(id, module).map_err(|error| match error {
                Error::InternalModule { module, .. } => Error::InternalModule {
                    module,
                    path: import.path.clone(),
                },
                other => other,
            })
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
        let library = ModuleId::library(&package);

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
        if self.sources.module(&whole).is_some() && !rest.is_empty() {
            reach(&whole)?;
            // The library module shares a scope with the top-level
            // modules, so a name in both would have two sources.
            if let [name] = rest
                && self.sources.module(&library).is_some()
                && self.read(&library)?.table.names(name)
            {
                return Err(Error::DuplicateName {
                    module: library,
                    name: name.clone(),
                });
            }
            if taken {
                return Err(duplicate());
            }
            let members = (*self.members(&whole)?).clone();
            scope.modules.insert(import.alias.clone(), (whole, members));
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
        reach(&parent)?;
        let members = self.members(&parent)?;
        let mut found = false;
        for namespace in [Namespace::Type, Namespace::Value, Namespace::Ability] {
            let Some(member) = members.0.get(&(namespace, name.clone())) else {
                continue;
            };
            if !member.public {
                return Err(Error::PrivateImport {
                    module: id.clone(),
                    path: import.path.clone(),
                });
            }
            if taken {
                return Err(duplicate());
            }
            found = true;
            scope
                .definitions
                .insert((namespace, import.alias.clone()), member.symbol.clone());
        }
        if found { Ok(()) } else { Err(unknown()) }
    }
}
