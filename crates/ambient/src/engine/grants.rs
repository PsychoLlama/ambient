//! What each package holds: the abilities those that depend on it let
//! it take. See "Grants" in `docs/spec/packages.md`.
//!
//! A grant is written as a path in the manifest that declares the
//! dependency, and means what a row there would: so it is read here,
//! where a module's members are known, and not where manifests are.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ambient_runtime::FAULT_PACKAGE;

use super::{Engine, Query};
use crate::builder::OWN_PACKAGE;
use crate::error::Error;
use crate::surface::Reference;
use crate::symbol::{ModuleId, Namespace, PackageId, Symbol};
use crate::workspace::Granting;

/// The module of the standard library whose abilities read the
/// machine a build is run on.
const BUILD: &str = "build";

/// What a package that is held holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Holding {
    abilities: BTreeSet<Symbol>,
    native: bool,
}

/// What each held package of a workspace holds.
pub(super) type Holdings = BTreeMap<PackageId, Holding>;

/// Whether `package` takes `ability` whatever it is granted: it is the
/// package's own, or the standard library's and reaches no world.
fn free(package: &PackageId, ability: &Symbol) -> bool {
    let of = &ability.module.package;
    let builds = ability
        .module
        .path
        .0
        .first()
        .is_some_and(|first| first == BUILD);
    of == package || (of.0 == FAULT_PACKAGE && !builds)
}

/// A grant as it is read: who made it, to whom, and of what.
struct Granted {
    from: PackageId,
    to: PackageId,
    abilities: Vec<Symbol>,
    native: bool,
}

/// The least each held package holds that satisfies every grant: one
/// is cut to what the package that made it holds, so this is found by
/// going round until nothing is added.
fn least(mut held: Holdings, granted: &[Granted]) -> Holdings {
    loop {
        let mut added = Vec::new();
        for grant in granted.iter().filter(|grant| held.contains_key(&grant.to)) {
            // A package that is not held is the workspace's own.
            let from = held.get(&grant.from);
            let passes = |ability: &&Symbol| {
                free(&grant.from, ability)
                    || from.is_none_or(|from| from.abilities.contains(ability))
            };
            let native = grant.native && from.is_none_or(|from| from.native);
            let to = &held[&grant.to];
            let news = |ability: &&Symbol| !to.abilities.contains(ability);
            let abilities = grant.abilities.iter().filter(passes).filter(news);
            let abilities: Vec<Symbol> = abilities.cloned().collect();
            if !abilities.is_empty() || (native && !to.native) {
                added.push((grant.to.clone(), abilities, native));
            }
        }
        if added.is_empty() {
            return held;
        }
        for (to, abilities, native) in added {
            let to = held.entry(to).or_default();
            to.abilities.extend(abilities);
            to.native |= native;
        }
    }
}

impl Engine<'_> {
    /// The abilities a grant's path names, written in a manifest of
    /// `from`: an ability, or every public one of a module.
    fn abilities_at(&self, from: &PackageId, path: &str) -> Option<Vec<Symbol>> {
        let (first, rest) = path.split_once('.').unwrap_or((path, ""));
        let package = match first {
            OWN_PACKAGE => from.clone(),
            first => self.sources.dependency(from, first)?,
        };
        let module = ModuleId::new(&package.0, rest);
        if self.sources.module(&module).is_some() {
            let members = self.members(&module).ok()?;
            let public = members.0.iter().filter(|((namespace, _), member)| {
                *namespace == Namespace::Ability && member.public
            });
            return Some(public.map(|(_, member)| member.symbol.clone()).collect());
        }
        let (module, name) = rest.rsplit_once('.').unwrap_or(("", rest));
        let members = self.members(&ModuleId::new(&package.0, module)).ok()?;
        let member = members.0.get(&(Namespace::Ability, name.to_owned()))?;
        member.public.then(|| vec![member.symbol.clone()])
    }

    /// A grant, with the abilities its paths name.
    fn granted(&self, grant: &Granting) -> Result<Granted, Error> {
        let mut abilities = Vec::new();
        for path in &grant.abilities {
            let named = self.abilities_at(&grant.from, path);
            abilities.extend(named.ok_or_else(|| Error::UnknownGrant {
                package: grant.from.clone(),
                dependency: grant.local.clone(),
                grant: path.clone(),
            })?);
        }
        Ok(Granted {
            from: grant.from.clone(),
            to: grant.to.clone(),
            abilities,
            native: grant.native,
        })
    }

    /// What each held package holds, worked out once.
    fn holdings(&self) -> Query<Holdings> {
        if let Some(holdings) = self.holdings.borrow().as_ref() {
            return holdings.clone();
        }
        let grants = self.sources.grants();
        let worked = || {
            let read: Result<Vec<Granted>, Error> =
                grants.iter().map(|grant| self.granted(grant)).collect();
            let read = read?;
            let mut held = Holdings::new();
            let ends = read.iter().flat_map(|grant| [&grant.from, &grant.to]);
            for package in ends.chain(self.sources.held().iter()) {
                if let Some(outright) = self.sources.holds(package) {
                    let holding = Holding {
                        abilities: outright.abilities.into_iter().collect(),
                        native: outright.native,
                    };
                    held.insert(package.clone(), holding);
                }
            }
            Ok(Arc::new(least(held, &read)))
        };
        let holdings = worked();
        *self.holdings.borrow_mut() = Some(holdings.clone());
        holdings
    }

    /// Whether `package` may take `ability`.
    pub(super) fn holds(&self, package: &PackageId, ability: &Symbol) -> Result<bool, Error> {
        if free(package, ability) || self.sources.holds(package).is_none() {
            return Ok(true);
        }
        let holdings = self.holdings()?;
        Ok(holdings
            .get(package)
            .is_some_and(|held| held.abilities.contains(ability)))
    }

    /// Everything `package` was granted, for a key: nothing, for a
    /// package that is not held.
    pub(super) fn granted_to(&self, package: &PackageId) -> Result<Option<Vec<Symbol>>, Error> {
        if self.sources.holds(package).is_none() {
            return Ok(None);
        }
        let holdings = self.holdings()?;
        let held = holdings.get(package);
        Ok(Some(
            held.map(|held| held.abilities.iter().cloned().collect())
                .unwrap_or_default(),
        ))
    }

    /// Whether a program may reach native code of `package`.
    ///
    /// # Errors
    ///
    /// A grant of the workspace names nothing.
    pub fn native(&self, package: &PackageId) -> Result<bool, Error> {
        if self.sources.holds(package).is_none() {
            return Ok(true);
        }
        Ok(self
            .holdings()?
            .get(package)
            .is_some_and(|held| held.native))
    }

    /// How what `error` says a package is not granted could be granted
    /// it: a line for each declaration that leads to the package.
    #[must_use]
    pub fn granting(&self, error: &Error) -> Vec<String> {
        let (Error::Takes { symbol, ability } | Error::Ungranted { symbol, ability }) = error
        else {
            return Vec::new();
        };
        let mut path = vec![ability.module.package.0.as_str()];
        path.extend(ability.module.path.0.iter().map(String::as_str));
        path.push(&ability.name);
        let path = path.join(".");
        let grants = self.sources.grants();
        // Only a package that holds it has it to grant.
        let could = |grant: &&Granting| {
            grant.to == symbol.module.package && self.holds(&grant.from, ability) == Ok(true)
        };
        let leads = grants.iter().filter(could);
        let line = |grant: &Granting| {
            let Granting { from, local, .. } = grant;
            format!(
                "\n  where `{}` declares `{local}`, grant it: grants = [\"{path}\"]",
                from.0
            )
        };
        leads.map(line).collect()
    }

    /// Fails if `symbol`, which takes `row` from whatever calls it,
    /// takes what its package is not granted.
    pub(super) fn held_to(&self, symbol: &Symbol, row: &[Reference]) -> Result<(), Error> {
        for ability in row {
            let Reference::Definition(ability) = ability else {
                continue;
            };
            if !self.holds(&symbol.module.package, ability)? {
                return Err(Error::Takes {
                    symbol: symbol.clone(),
                    ability: ability.clone(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Granted, Holding, Holdings, free, least};
    use crate::symbol::{ModuleId, PackageId, Symbol};

    fn read() -> Symbol {
        ModuleId::new("system", "fs").ability("read")
    }

    /// A grant of reading from one package to another.
    fn grant(from: &str, to: &str) -> Granted {
        Granted {
            from: PackageId(from.to_owned()),
            to: PackageId(to.to_owned()),
            abilities: vec![read()],
            native: true,
        }
    }

    /// What those packages hold after `granted`, each held from the
    /// start to nothing.
    fn held(packages: &[&str], granted: &[Granted]) -> Vec<bool> {
        let package = |name: &&str| PackageId((*name).to_owned());
        let nothing = packages
            .iter()
            .map(|name| (package(name), Holding::default()));
        let least = least(nothing.collect::<Holdings>(), granted);
        let holds = |name: &&str| {
            let held = &least[&package(name)];
            held.native && held.abilities.contains(&read())
        };
        packages.iter().map(holds).collect()
    }

    #[test]
    fn a_package_takes_its_own_abilities_and_what_reaches_no_world() {
        let app = PackageId("app".to_owned());
        let of = |package: &str, module: &str| ModuleId::new(package, module).ability("it");

        assert!(free(&app, &of("app", "main")));
        assert!(free(&app, &of("ambient", "errors")));
        assert!(!free(&app, &of("ambient", "build")));
        assert!(!free(&app, &of("system", "fs")));
    }

    #[test]
    fn a_grant_reaches_what_is_beneath_only_through_what_holds_it() {
        let chain = [grant("b", "c"), grant("app", "b")];
        assert_eq!(held(&["b", "c"], &chain), [true, true]);
        // What `b` was never granted, it cannot pass on.
        assert_eq!(held(&["b", "c"], &chain[..1]), [false, false]);
    }

    #[test]
    fn packages_that_depend_on_each_other_hold_only_what_came_from_outside() {
        let ring = [grant("b", "c"), grant("c", "b")];
        assert_eq!(held(&["b", "c"], &ring), [false, false]);
    }
}
