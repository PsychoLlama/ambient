//! Externs and targets. See `docs/spec/externs.md` and `docs/spec/targets.md`.

use std::collections::BTreeMap;

use super::Checker;
use super::generics::generic_type;
use crate::core::{Open, Tree};
use crate::error::Error;
use crate::surface::{Extern, Reference};
use crate::symbol::Symbol;
use crate::types::{AbilityInfo, Env, Ty, abilities, layers, trace};

/// Checks a native handler: it takes what its ability takes, and the
/// trace of the abort if its ability does not return.
///
/// # Errors
///
/// Fails if what it handles is no ability, or it takes a function that
/// performs anything.
pub fn check_extern(
    symbol: &Symbol,
    native: &Extern<Reference>,
    package: &str,
    env: &Env,
) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    let ability = abilities(std::slice::from_ref(&native.ability)).pop();
    let info = ability
        .as_ref()
        .and_then(|ability| env.abilities.get(ability));
    let (Some(ability), Some(info)) = (ability.clone(), info) else {
        return Err(Error::UnknownSymbol(symbol.clone()));
    };
    let mut params = info.params.clone();
    // One that does not return is where an abort ends, and is handed
    // its trace.
    if info.ret == Ty::Never {
        params.push(env.normalize(&trace()));
    }
    let (ret, row) = (info.ret.clone(), abilities(&native.row));
    if params.iter().any(takes_bindings) {
        return Err(Error::NativeRow(symbol.clone()));
    }
    let params = params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&ret);
    let handles = checker.slot(&ability);
    let row = checker.row(&row);
    let suspends = native.suspends;
    Ok(Open {
        tree: Tree::Extern {
            package: package.to_owned(),
            path: symbol.identity(),
            params,
            ret,
            handles,
            suspends,
            row,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
        annotations: Vec::new(),
    })
}

/// Whether a type an extern takes holds a function type with a row.
/// Native code calls a function with nothing bound, so what it is
/// handed must be bound already: see "Calling back" in
/// `docs/spec/externs.md`.
fn takes_bindings(ty: &Ty) -> bool {
    let found = std::cell::Cell::new(false);
    let _ = ty.rewrite(0, &|ty, _| {
        if matches!(ty, Ty::Function { row, .. } if !row.is_empty()) {
            found.set(true);
        }
        None
    });
    found.get()
}

/// The type a root handler for an ability must fit, given the one
/// `found` for it. A handler may take whatever the target offers. One
/// for an ability that does not return may return anything, and may
/// take the trace of the abort after the ability's own parameters: see
/// "Abort" in `docs/spec/handlers.md`.
fn root_handler(env: &Env, info: &AbilityInfo, offered: &[Reference], found: &Ty) -> Ty {
    let expected =
        |info: &AbilityInfo, ret: Ty| env.normalize(&info.handler(ret, offered.to_vec()));
    if info.ret != Ty::Never {
        return expected(info, info.ret.clone());
    }
    let mut traced = info.clone();
    traced.params.push(env.normalize(&trace()));
    let traced = expected(&traced, Ty::Unknown);
    if env.subtype(found, &traced) {
        traced
    } else {
        expected(info, Ty::Unknown)
    }
}

/// Checks a target, given every binding it has: its own, and those of the
/// targets it extends.
///
/// # Errors
///
/// Fails if a handler does not fit its ability, takes an ability the
/// target does not offer, or needs itself.
pub fn check_target(
    symbol: &Symbol,
    bindings: &[(Symbol, Symbol)],
    env: &Env,
) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    let offered: Vec<Reference> = bindings
        .iter()
        .map(|(ability, _)| Reference::Definition(ability.clone()))
        .collect();
    let mut rows = BTreeMap::new();
    for (ability, handler) in bindings {
        let not_handler = || Error::NotHandler {
            symbol: symbol.clone(),
            handler: handler.clone(),
        };
        let info = env.abilities.get(ability).ok_or_else(not_handler)?;
        let signature = env.functions.get(handler).ok_or_else(not_handler)?;
        let found = env.normalize(&generic_type(signature));
        let expected = root_handler(env, info, &offered, &found);
        if !env.subtype(&found, &expected) {
            return Err(checker.mismatch(&expected, &found));
        }
        rows.insert(ability.clone(), signature.row.clone());
    }
    if layers(&rows).is_none() {
        return Err(Error::TargetCycle(symbol.clone()));
    }

    let handlers: BTreeMap<&Symbol, &Symbol> = bindings.iter().map(|(a, h)| (a, h)).collect();
    let bound: Vec<Symbol> = bindings
        .iter()
        .map(|(ability, _)| ability.clone())
        .collect();
    let mut tree = Vec::new();
    // In row order, which is by tag.
    for ability in checker.row(&bound) {
        let name = &checker.frame().slots[ability.0];
        let handler = handlers.get(name).copied().cloned();
        if let Some(handler) = handler {
            tree.push((ability, checker.slot(&handler)));
        }
    }
    Ok(Open {
        tree: Tree::Target { bindings: tree },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
        annotations: Vec::new(),
    })
}
