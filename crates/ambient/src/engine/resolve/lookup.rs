//! What a name means where it is written: a definition of the module,
//! one it imports, or one of a prelude. See `docs/spec/symbols.md`.

use super::Resolver;
use crate::engine::scope::{Lazy, Member, Members};
use crate::error::Error;
use crate::surface::Path;
use crate::symbol::{ModuleId, Namespace, Symbol};

impl Resolver<'_> {
    /// The members of a generated module, if they were given. One
    /// whose were not is wanted, and whatever asked finds nothing.
    fn made(&self, module: &ModuleId) -> Option<&Members> {
        let given = self.given.get(module);
        if given.is_none() && !self.wanted.borrow().contains(module) {
            self.wanted.borrow_mut().push(module.clone());
        }
        given.map(|members| &**members)
    }

    /// The members of the module imported as `alias`, if one is.
    pub(super) fn module(&self, alias: &str) -> Option<&Members> {
        match self.scope.generated.get(alias) {
            Some(Lazy::Module(module)) => self.made(module),
            _ => self.scope.modules.get(alias).map(|(_, members)| members),
        }
    }

    /// The module imported as `alias`, if one is, generated or not.
    pub(super) fn imported(&self, alias: &str) -> Option<ModuleId> {
        match self.scope.generated.get(alias) {
            Some(Lazy::Module(module)) => Some(module.clone()),
            _ => self
                .scope
                .modules
                .get(alias)
                .map(|(module, _)| module.clone()),
        }
    }

    /// A definition named directly, or through one imported module.
    pub(super) fn lookup(&self, namespace: Namespace, path: &Path) -> Result<Symbol, Error> {
        let unknown = || Error::UnknownName {
            symbol: self.symbol.clone(),
            path: path.clone(),
        };
        let public = |member: &Member| {
            if member.public {
                Ok(member.symbol.clone())
            } else {
                Err(Error::PrivateName {
                    symbol: self.symbol.clone(),
                    path: path.clone(),
                })
            }
        };
        match path.0.as_slice() {
            [name] => {
                let key = (namespace, name.clone());
                if let Some(symbol) = self.scope.definitions.get(&key) {
                    return Ok(symbol.clone());
                }
                // A name imported from a generated module comes
                // before a prelude's, as any use does.
                if let Some(Lazy::Member(module, member)) = self.scope.generated.get(name) {
                    let members = self.made(module).ok_or_else(unknown)?;
                    if let Some(member) = members.0.get(&(namespace, member.clone())) {
                        return public(member);
                    }
                }
                self.scope.preludes.get(&key).cloned().ok_or_else(unknown)
            }
            [module, name] => {
                let members = self.module(module).ok_or_else(unknown)?;
                let member = members.0.get(&(namespace, name.clone()));
                public(member.ok_or_else(unknown)?)
            }
            _ => Err(unknown()),
        }
    }
}
