//! Externs and targets. See `spec/externs.md` and `spec/targets.md`.

use std::collections::BTreeMap;

use super::Checker;
use super::generics::function_type;
use crate::core::{Open, Tree};
use crate::error::Error;
use crate::surface::{Extern, ExternKind, Reference};
use crate::symbol::Symbol;
use crate::types::{Env, Ty, abilities, layers};

/// Checks an extern: a tag and a signature.
///
/// # Errors
///
/// Fails if a type in the signature is ill-formed.
pub fn check_extern(symbol: &Symbol, native: &Extern<Reference>, env: &Env) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    let (params, ret, handles, row) = match &native.kind {
        ExternKind::Function { params, ret } => {
            let mut types = Vec::new();
            for param in params {
                checker.well_formed(&param.ty)?;
                types.push(env.normalize(&param.ty));
            }
            checker.well_formed(ret)?;
            (types, env.normalize(ret), None, Vec::new())
        }
        // A native handler takes what its ability takes.
        ExternKind::Handler { ability, row } => {
            let ability = abilities(std::slice::from_ref(ability)).pop();
            let info = ability
                .as_ref()
                .and_then(|ability| env.abilities.get(ability));
            let (Some(ability), Some(info)) = (ability.clone(), info) else {
                return Err(Error::UnknownSymbol(symbol.clone()));
            };
            (
                info.params.clone(),
                info.ret.clone(),
                Some(ability),
                abilities(row),
            )
        }
    };
    let params = params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&ret);
    let handles = handles.map(|ability| checker.slot(&ability));
    let row = checker.row(&row);
    Ok(Open {
        tree: Tree::Extern {
            package: symbol.module.package.0.clone(),
            tag: native.tag.clone(),
            params,
            ret,
            handles,
            row,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
    })
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
        let signature = env
            .functions
            .get(handler)
            .filter(|signature| signature.type_params.is_empty())
            .ok_or_else(not_handler)?;
        // A handler may take whatever the target offers, and one for an
        // ability that does not return may return anything.
        let ret = match &info.ret {
            Ty::Never => Ty::Unknown,
            ret => ret.clone(),
        };
        let expected = Ty::Function {
            params: info.params.clone(),
            ret: Box::new(ret),
            row: offered.clone(),
        };
        let found = env.normalize(&function_type(signature, &[]));
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
    })
}
