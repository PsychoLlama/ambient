//! Default bindings: the native handlers a package says answer its own
//! abilities under a target. See "Default bindings" in
//! `docs/spec/targets.md`.
//!
//! A default is found by its ability: only the package that declares
//! the ability gives one, so nothing else is read to find it. An
//! ability of a generated module is given one by a handler beside it,
//! so only that module's call is run to find it.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Engine, Query, memo};
use crate::error::Error;
use crate::surface::{DefinitionKind, Extern, ExternKind, Reference};
use crate::symbol::{ModuleId, Namespace, PackageId, Symbol};
use crate::types::abilities;

/// A native handler that is its ability's default under `targets`.
#[derive(Debug)]
pub(super) struct Defaulted {
    ability: Symbol,
    handler: Symbol,
    targets: Vec<Symbol>,
}

/// What a target binds for the abilities a program reaches: for each,
/// its handler and what that handler takes.
pub(super) type Bound = BTreeMap<Symbol, (Symbol, Vec<Symbol>)>;

impl Engine<'_> {
    /// Every default a package gives, in the order it is written.
    fn defaults(&self, package: &PackageId) -> Query<Vec<Defaulted>> {
        memo(&self.defaults, package, || {
            let mut found = Vec::new();
            for module in self.sources.modules(package) {
                let read = self.read(&module)?;
                // Only what says a target is resolved: a package's
                // other definitions are not read to bind one.
                let says = |kind: &DefinitionKind| {
                    matches!(kind, DefinitionKind::Extern(Extern {
                        kind: ExternKind::Handler { targets, .. }, ..
                    }) if !targets.is_empty())
                };
                for definition in read.definitions.iter().filter(|one| says(&one.kind)) {
                    let handler = module.symbol(Namespace::Value, &definition.name);
                    let resolved = self.resolve(&handler)?;
                    found.extend(defaulted(handler, &resolved.definition.kind));
                }
            }
            Ok(Arc::new(found))
        })
    }

    /// Every default the handlers of a generated module give; none, for
    /// a module that was written.
    fn generated_defaults(&self, module: &ModuleId) -> Result<Vec<Defaulted>, Error> {
        let made = self.made(module)?;
        let handler = |name: &str| module.symbol(Namespace::Value, name);
        Ok(made
            .iter()
            .filter_map(|made| defaulted(handler(&made.name), &made.kind))
            .collect())
    }

    /// The default binding of `ability` under `target`: the native
    /// handler of the ability's package that names the target or one
    /// it extends.
    ///
    /// # Errors
    ///
    /// Fails if two handlers are, or as [`Self::extends`].
    pub(super) fn default(
        &self,
        target: &Symbol,
        ability: &Symbol,
    ) -> Result<Option<Symbol>, Error> {
        let defaults = self.defaults(&ability.module.package)?;
        let generated = self.generated_defaults(&ability.module)?;
        let mut found: Option<&Symbol> = None;
        let all = defaults.iter().chain(&generated);
        for default in all.filter(|one| one.ability == *ability) {
            let mut under = false;
            for base in &default.targets {
                under = under || self.extends(target, base)?;
            }
            if !under {
                continue;
            }
            if found.is_some_and(|other| *other != default.handler) {
                return Err(Error::TargetConflict {
                    symbol: target.clone(),
                    ability: ability.clone(),
                });
            }
            found = Some(&default.handler);
        }
        Ok(found.cloned())
    }

    /// What `target` binds for each of `asked` and for whatever the
    /// handlers bound take in turn: what it lists, and the default
    /// where it lists none. An ability nothing binds is left out.
    ///
    /// # Errors
    ///
    /// Fails if what is bound has no signature, or as [`Self::default`].
    pub(super) fn bound(&self, target: &Symbol, mut asked: Vec<Symbol>) -> Result<Bound, Error> {
        let listed: BTreeMap<Symbol, Symbol> = self.bindings(target)?.into_iter().collect();
        let mut bound = Bound::new();
        while let Some(ability) = asked.pop() {
            if bound.contains_key(&ability) {
                continue;
            }
            let handler = match listed.get(&ability) {
                Some(handler) => Some(handler.clone()),
                None => self.default(target, &ability)?,
            };
            let Some(handler) = handler else {
                continue;
            };
            let Some((signature, _)) = self.signature(&handler)? else {
                return Err(Error::NotHandler {
                    symbol: target.clone(),
                    handler,
                });
            };
            asked.extend(signature.row.iter().cloned());
            bound.insert(ability, (handler, signature.row));
        }
        Ok(bound)
    }

    /// What a target is checked as binding: what it lists, and the
    /// defaults its handlers take in turn.
    ///
    /// # Errors
    ///
    /// As [`Self::bound`].
    pub(super) fn offered(&self, target: &Symbol) -> Result<Vec<(Symbol, Symbol)>, Error> {
        let listed = self.bindings(target)?;
        let asked = listed.iter().map(|(ability, _)| ability.clone()).collect();
        let bound = self.bound(target, asked)?.into_iter();
        Ok(bound
            .map(|(ability, (handler, _))| (ability, handler))
            .collect())
    }

    /// Fails unless a native handler's defaults are its to give: each
    /// under a target, and for an ability of its own package.
    pub(super) fn defaulting(
        &self,
        symbol: &Symbol,
        native: &ExternKind<crate::surface::Reference>,
    ) -> Result<(), Error> {
        let ExternKind::Handler {
            ability, targets, ..
        } = native
        else {
            return Ok(());
        };
        for target in abilities(targets) {
            if !matches!(
                self.resolve(&target)?.definition.kind,
                DefinitionKind::Target(_)
            ) {
                return Err(Error::NotTarget(target));
            }
        }
        // A generated handler is found only beside its ability, so it
        // gives a default only for an ability of its own module.
        let generated = self.generates(&symbol.module)?;
        let foreign = abilities(std::slice::from_ref(ability))
            .pop()
            .filter(|ability| {
                ability.module.package != symbol.module.package
                    || (generated && ability.module != symbol.module)
            });
        match foreign {
            Some(ability) if !targets.is_empty() => Err(Error::ForeignDefault {
                symbol: symbol.clone(),
                ability,
            }),
            _ => Ok(()),
        }
    }
}

/// The default a definition gives, if it is a native handler that
/// names targets.
fn defaulted(handler: Symbol, kind: &DefinitionKind<Reference>) -> Option<Defaulted> {
    let DefinitionKind::Extern(Extern {
        kind: ExternKind::Handler {
            ability, targets, ..
        },
        ..
    }) = kind
    else {
        return None;
    };
    let ability = abilities(std::slice::from_ref(ability)).pop()?;
    let targets = abilities(targets);
    (!targets.is_empty()).then_some(Defaulted {
        ability,
        handler,
        targets,
    })
}
