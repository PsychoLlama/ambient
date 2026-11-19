//! The core checker: whether a core tree is one checking could have made.
//! See `spec/exchange.md`.
//!
//! Everything after checking trusts a core tree: lowering reads a field
//! without asking whether the record has it, and calls a handler without
//! asking whether one is bound. A tree that did not come from this
//! compiler's own checker has to earn that trust, and this is where it
//! does. Nothing here looks at a surface tree, so it works on a definition
//! that arrived with no source.
//!
//! A core tree holds every type a later stage needs, so there is nothing
//! to infer: each node's type follows from the node and the types of what
//! is inside it. The rules are those of [`crate::checker`], read off the
//! tree it produces, and both ask [`Env`] the same questions.
//!
//! A definition is verified against the headers of what its slots refer
//! to, and each of those is verified on its own, against its own header.

mod calls;
mod expr;
mod patterns;
mod tasks;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::checker::generic_type;
use crate::core::{self, Open, Slot, Tree};
use crate::surface::Reference;
use crate::symbol::Symbol;
use crate::types::{AbilityInfo, ContractInfo, Env, Signature, Ty, TypeInfo, layers};

/// Why a core tree is not one checking could have made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Invalid {
    /// A slot is outside the table, or refers to the wrong kind of
    /// definition, or the table names one definition twice.
    Slot,
    /// A type is ill-formed, or is not written in the one form it has.
    Type,
    /// A local or a captured value that is not in scope is read.
    Local,
    /// A value's type is not a subtype of the one required of it.
    Mismatch { expected: String, found: String },
    /// Something is given the wrong number of arguments.
    Arity,
    /// An ability is used where nothing binds it.
    Unbound(String),
    /// A match lets a value through, or has an arm nothing reaches.
    Match,
    /// A node is not put together as checking puts one together.
    Shape(String),
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Slot => write!(f, "a slot refers to nothing it could"),
            Self::Type => write!(f, "a type is ill-formed or not in canonical form"),
            Self::Local => write!(f, "a local is read where it is not in scope"),
            Self::Mismatch { expected, found } => {
                write!(f, "expected `{expected}`, found `{found}`")
            }
            Self::Arity => write!(f, "the wrong number of arguments"),
            Self::Unbound(ability) => write!(f, "`{ability}` is not bound"),
            Self::Match => write!(f, "a match is not exhaustive, or has an unreachable arm"),
            Self::Shape(what) => write!(f, "{what}"),
        }
    }
}

fn shape<T>(what: &str) -> Result<T, Invalid> {
    Err(Invalid::Shape(what.to_owned()))
}

/// The definition a symbol names, if there is one.
pub type Lookup<'a> = &'a dyn Fn(&Symbol) -> Option<Arc<Open>>;

/// What a closure over a function must supply.
#[derive(Default)]
struct Held {
    /// The type of each value it captures.
    captures: Vec<Ty>,
    /// The abilities whose bindings it captures.
    bindings: Vec<Symbol>,
}

/// Everything verifying one definition reads about the others.
struct Verifier<'a> {
    slots: &'a [Symbol],
    env: Env,
    /// What each function among the slots captures.
    held: BTreeMap<Symbol, Held>,
    /// The targets among the slots.
    targets: BTreeSet<Symbol>,
    /// Whether this is a program's root, which alone may name a native
    /// handler.
    root: bool,
}

/// Verifies one definition. The lambdas written in it are definitions of
/// their own, and are not looked at.
///
/// With `root`, the definition may name a native handler: only a program's
/// root does, and nobody receives one.
///
/// # Errors
///
/// Fails if checking could not have produced the tree.
pub fn verify(open: &Open, root: bool, lookup: Lookup<'_>) -> Result<(), Invalid> {
    let unique: BTreeSet<&Symbol> = open.slots.iter().collect();
    if unique.len() != open.slots.len() {
        return Err(Invalid::Slot);
    }
    let mut verifier = Verifier {
        slots: &open.slots,
        env: Env::default(),
        held: BTreeMap::new(),
        targets: BTreeSet::new(),
        root,
    };
    verifier.gather(lookup)?;
    match &open.tree {
        Tree::Type { params, ty } => {
            verifier.env.bounds = vec![Ty::Unknown; *params];
            verifier.sound(ty).map(drop)
        }
        Tree::Ability {
            type_params,
            params,
            ret,
            ..
        } => {
            verifier.env.bounds = vec![Ty::Unknown; type_params.len()];
            verifier.sound_all(type_params)?;
            verifier.sound_all(params)?;
            let ret = verifier.sound(ret)?;
            if ret == Ty::Never && !type_params.is_empty() {
                return shape("an ability that does not return has type parameters");
            }
            Ok(())
        }
        Tree::Contract { ty, .. } => verifier.sound(ty).map(drop),
        Tree::Extern {
            params,
            ret,
            handles,
            row,
            ..
        } => verifier.external(params, ret, *handles, row),
        Tree::Target { bindings } => verifier.target(bindings),
        Tree::Function(function) => verifier.function(function),
    }
}

/// The definitions `types` name.
fn definitions(types: &[Ty]) -> Vec<Symbol> {
    let mut found = Vec::new();
    for ty in types {
        ty.names(&mut |name| {
            if let Reference::Definition(symbol) = name {
                found.push(symbol.clone());
            }
        });
    }
    found
}

impl Verifier<'_> {
    fn symbol(&self, slot: Slot) -> Result<&Symbol, Invalid> {
        self.slots.get(slot.0).ok_or(Invalid::Slot)
    }

    /// The ability a slot refers to.
    fn ability(&self, slot: Slot) -> Result<(&Symbol, &AbilityInfo), Invalid> {
        let symbol = self.symbol(slot)?;
        let info = self.env.abilities.get(symbol).ok_or(Invalid::Slot)?;
        Ok((symbol, info))
    }

    /// The abilities a row's slots refer to, which must be in the one
    /// order a row has.
    fn row(&self, row: &[Slot]) -> Result<Vec<Symbol>, Invalid> {
        let mut abilities = Vec::new();
        for slot in row {
            abilities.push(self.ability(*slot)?.0.clone());
        }
        if self.env.row(&abilities) != abilities {
            return shape("a row is not ordered by identity");
        }
        Ok(abilities)
    }

    fn mismatch(&self, expected: &Ty, found: &Ty) -> Invalid {
        Invalid::Mismatch {
            expected: self.env.show(expected),
            found: self.env.show(found),
        }
    }

    /// Fills the environment with the header of everything the slots
    /// reach: what each refers to, and whatever those headers name.
    fn gather(&mut self, lookup: Lookup<'_>) -> Result<(), Invalid> {
        let mut pending: Vec<Symbol> = self.slots.to_vec();
        let mut seen = BTreeSet::new();
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            let target = lookup(&symbol).ok_or(Invalid::Slot)?;
            let raise = |ty: &core::Type| types::raise(ty, &target.slots);
            let all = |types: &[core::Type]| types.iter().map(raise).collect::<Result<Vec<_>, _>>();
            let named = |slots: &[Slot]| -> Result<Vec<Symbol>, Invalid> {
                let symbol = |slot: &Slot| target.slots.get(slot.0).cloned().ok_or(Invalid::Slot);
                slots.iter().map(symbol).collect()
            };
            let mut header: Vec<Ty> = Vec::new();
            match &target.tree {
                Tree::Type { params, ty } => {
                    let info = TypeInfo {
                        params: *params,
                        ty: raise(ty)?,
                        recursive: true,
                        variance: Vec::new(),
                        finite: true,
                    };
                    header.push(info.ty.clone());
                    self.env.types.insert(symbol, info);
                }
                Tree::Ability {
                    package,
                    path,
                    type_params,
                    params,
                    ret,
                } => {
                    let info = AbilityInfo {
                        identity: (package.clone(), path.clone()),
                        type_params: all(type_params)?,
                        params: all(params)?,
                        ret: raise(ret)?,
                    };
                    header.extend(info.params.iter().cloned());
                    header.push(info.ret.clone());
                    self.env.abilities.insert(symbol, info);
                }
                Tree::Contract { package, path, ty } => {
                    let info = ContractInfo {
                        identity: (package.clone(), path.clone()),
                        ty: raise(ty)?,
                    };
                    header.push(info.ty.clone());
                    self.env.contracts.insert(symbol, info);
                }
                Tree::Extern {
                    params,
                    ret,
                    handles,
                    row,
                    ..
                } => {
                    let signature = Signature {
                        type_params: Vec::new(),
                        params: all(params)?,
                        ret: raise(ret)?,
                        row: named(row)?,
                        native: handles.is_some(),
                    };
                    pending.extend(signature.row.iter().cloned());
                    header.extend(signature.params.iter().cloned());
                    header.push(signature.ret.clone());
                    self.held.insert(symbol.clone(), Held::default());
                    self.env.functions.insert(symbol, signature);
                }
                Tree::Function(function) => {
                    let signature = Signature {
                        type_params: all(&function.type_params)?,
                        params: all(&function.params)?,
                        ret: raise(&function.ret)?,
                        row: named(&function.row)?,
                        native: false,
                    };
                    let held = Held {
                        captures: all(&function.captures)?,
                        bindings: named(&function.bindings)?,
                    };
                    pending.extend(signature.row.iter().cloned());
                    pending.extend(held.bindings.iter().cloned());
                    header.extend(signature.type_params.iter().cloned());
                    header.extend(signature.params.iter().cloned());
                    header.extend(held.captures.iter().cloned());
                    header.push(signature.ret.clone());
                    self.held.insert(symbol.clone(), held);
                    self.env.functions.insert(symbol, signature);
                }
                Tree::Target { .. } => {
                    self.targets.insert(symbol);
                }
            }
            pending.extend(definitions(&header));
        }
        self.normalize();
        Ok(())
    }

    /// Puts every header in normal form, which is how they are read.
    fn normalize(&mut self) {
        // How each recursive type varies is known before any two
        // types are compared.
        self.env.settle();
        let env = &self.env;
        let all = |types: &[Ty]| -> Vec<Ty> { types.iter().map(|ty| env.normalize(ty)).collect() };
        let abilities = env
            .abilities
            .iter()
            .map(|(symbol, info)| {
                let info = AbilityInfo {
                    identity: info.identity.clone(),
                    type_params: all(&info.type_params),
                    params: all(&info.params),
                    ret: env.normalize(&info.ret),
                };
                (symbol.clone(), info)
            })
            .collect();
        let functions = env
            .functions
            .iter()
            .map(|(symbol, signature)| {
                let signature = Signature {
                    type_params: all(&signature.type_params),
                    params: all(&signature.params),
                    ret: env.normalize(&signature.ret),
                    row: signature.row.clone(),
                    native: signature.native,
                };
                (symbol.clone(), signature)
            })
            .collect();
        let keys = env.contracts.iter().map(|(symbol, info)| {
            let ty = env.normalize(&info.ty);
            let identity = info.identity.clone();
            (symbol.clone(), ContractInfo { identity, ty })
        });
        let keys = keys.collect();
        for held in self.held.values_mut() {
            held.captures = all(&held.captures);
        }
        self.env.contracts = keys;
        self.env.abilities = abilities;
        self.env.functions = functions;
    }

    /// An extern: a signature, and, for a native handler, the ability
    /// whose signature it is.
    fn external(
        &self,
        params: &[core::Type],
        ret: &core::Type,
        handles: Option<Slot>,
        row: &[Slot],
    ) -> Result<(), Invalid> {
        if params.iter().any(core::Type::takes_bindings) {
            return shape("an extern takes a function whose type has a row");
        }
        let params = self.sound_all(params)?;
        let ret = self.sound(ret)?;
        self.row(row)?;
        if let Some(ability) = handles {
            let (_, info) = self.ability(ability)?;
            // One for an ability that does not return is handed the
            // trace of the abort it ends.
            let mut takes = info.params.clone();
            if info.ret == Ty::Never {
                takes.push(self.env.normalize(&crate::types::trace()));
            }
            if takes != params || info.ret != ret {
                return shape("a native handler does not take what its ability takes");
            }
        }
        Ok(())
    }

    /// A target: each handler fits its ability and takes only what the
    /// target offers, and none needs itself.
    fn target(&self, bindings: &[(Slot, Slot)]) -> Result<(), Invalid> {
        let abilities: Vec<Slot> = bindings.iter().map(|(ability, _)| *ability).collect();
        let offered = self.row(&abilities)?;
        let mut rows = BTreeMap::new();
        for (ability, handler) in bindings {
            let (ability, info) = self.ability(*ability)?;
            let handler = self.symbol(*handler)?;
            let signature = self.env.functions.get(handler).ok_or(Invalid::Slot)?;
            let held = self.held.get(handler).ok_or(Invalid::Slot)?;
            if !held.captures.is_empty() || !held.bindings.is_empty() {
                return shape("a target's handler is a function that stands alone");
            }
            let ret = match &info.ret {
                Ty::Never => Ty::Unknown,
                ret => ret.clone(),
            };
            let row = offered.iter().cloned().map(Reference::Definition).collect();
            let expected = self.env.normalize(&info.handler(ret, row));
            let found = self.env.normalize(&generic_type(signature));
            if !self.env.subtype(&found, &expected) {
                return Err(self.mismatch(&expected, &found));
            }
            rows.insert(ability.clone(), signature.row.clone());
        }
        if layers(&rows).is_none() {
            return shape("a target's handlers need each other in a circle");
        }
        Ok(())
    }
}
