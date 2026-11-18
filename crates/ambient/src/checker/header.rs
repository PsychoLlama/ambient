//! Headers and type definitions: what is checked without looking at a
//! body.

use super::Checker;
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::surface::{Ability, Function, KeyDefinition, Reference, TypeDefinition};
use crate::symbol::Symbol;
use crate::types::{Env, Ty, abilities};

/// Checks a type definition.
///
/// # Errors
///
/// Fails if the type is ill-formed, or recursive without passing
/// through a constructor.
pub fn check_type(
    symbol: &Symbol,
    definition: &TypeDefinition<Reference>,
    env: &Env,
) -> Result<Open, Error> {
    contractive(symbol, &definition.ty, env, &mut vec![symbol.clone()])?;
    let mut checker = Checker::new(symbol, env);
    checker.well_formed(&definition.ty)?;
    let ty = checker.lower(&env.normalize(&definition.ty));
    Ok(Open {
        tree: Tree::Type {
            params: definition.params.len(),
            ty,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
        annotations: Vec::new(),
    })
}

/// The canonical encoding of a function's header, and the slots it uses.
///
/// # Errors
///
/// Fails if a type in the header is ill-formed.
pub fn encode_header(
    symbol: &Symbol,
    function: &Function<Reference>,
    mut env: Env,
) -> Result<(Vec<u8>, Vec<Symbol>), Error> {
    let header = header(symbol, function, &mut env)?;
    let mut checker = Checker::new(symbol, &env);
    let type_params: Vec<_> = env.bounds.iter().map(|ty| checker.lower(ty)).collect();
    let params: Vec<_> = header.params.iter().map(|ty| checker.lower(ty)).collect();
    let ret = checker.lower(&header.ret);
    let row = checker.row(&header.row);
    let encoding = core::encode_signature(&type_params, &params, &ret, &row);
    Ok((encoding, checker.finish().slots))
}

/// Checks an ability: a tag and a signature.
///
/// # Errors
///
/// Fails if a type in the signature is ill-formed.
pub fn check_ability(
    symbol: &Symbol,
    ability: &Ability<Reference>,
    package: &str,
    env: &Env,
) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    let mut type_params = Vec::new();
    for bound in ability.type_params.iter().map(|param| &param.bound) {
        type_params.push(match bound {
            Some(bound) => {
                checker.well_formed(bound)?;
                checker.lower(&env.normalize(bound))
            }
            None => core::Type::Unknown,
        });
    }
    let mut params = Vec::new();
    for param in &ability.params {
        checker.well_formed(&param.ty)?;
        params.push(checker.lower(&env.normalize(&param.ty)));
    }
    checker.well_formed(&ability.ret)?;
    let ret = env.normalize(&ability.ret);
    // Nothing comes back from an ability that does not return, so a
    // type parameter of one would relate nothing to what was passed.
    if ret == Ty::Never && !type_params.is_empty() {
        return Err(Error::GenericAbort(symbol.clone()));
    }
    let ret = checker.lower(&ret);
    Ok(Open {
        tree: Tree::Ability {
            package: package.to_owned(),
            path: symbol.identity(),
            type_params,
            params,
            ret,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
        annotations: Vec::new(),
    })
}

/// Checks a key and lowers it to its core tree.
///
/// # Errors
///
/// Fails if its type is ill-formed.
pub fn check_key(
    symbol: &Symbol,
    key: &KeyDefinition<Reference>,
    package: &str,
    env: &Env,
) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    checker.well_formed(&key.ty)?;
    let ty = checker.lower(&env.normalize(&key.ty));
    Ok(Open {
        tree: Tree::Key {
            package: package.to_owned(),
            path: symbol.identity(),
            ty,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
        annotations: Vec::new(),
    })
}

pub(super) struct Header {
    pub(super) params: Vec<Ty>,
    pub(super) ret: Ty,
    pub(super) row: Vec<Symbol>,
}

/// Validates and normalizes a header, and records its bounds in `env`.
pub(super) fn header(
    symbol: &Symbol,
    function: &Function<Reference>,
    env: &mut Env,
) -> Result<Header, Error> {
    let checker = Checker::new(symbol, env);
    let mut bounds = Vec::new();
    for param in &function.type_params {
        bounds.push(match &param.bound {
            Some(bound) => {
                checker.well_formed(bound)?;
                env.normalize(bound)
            }
            None => Ty::Unknown,
        });
    }
    let mut params = Vec::new();
    for param in &function.params {
        checker.well_formed(&param.ty)?;
        params.push(env.normalize(&param.ty));
    }
    checker.well_formed(&function.ret)?;
    let ret = env.normalize(&function.ret);
    env.bounds = bounds;
    let row = abilities(&function.row);
    Ok(Header { params, ret, row })
}

/// Calls `visit` with `ty` and every type inside it.
pub(super) fn visit_types(ty: &Ty, visit: &mut impl FnMut(&Ty)) {
    visit(ty);
    match ty {
        Ty::Record(fields) => fields.iter().for_each(|(_, ty)| visit_types(ty, visit)),
        Ty::Tuple(members) | Ty::Union(members) | Ty::Named(_, members) => {
            for member in members {
                visit_types(member, visit);
            }
        }
        Ty::List(member) | Ty::Cell(member) | Ty::Iterator(member) | Ty::Branded(member, _) => {
            visit_types(member, visit);
        }
        Ty::Function { params, ret, .. } => {
            for param in params {
                visit_types(param, visit);
            }
            visit_types(ret, visit);
        }
        Ty::Generic { bounds, function } => {
            for bound in bounds {
                visit_types(bound, visit);
            }
            visit_types(function, visit);
        }
        _ => {}
    }
}

/// Fails if following names from `ty`, without passing through a
/// constructor, leads back to a name already being followed.
fn contractive(
    symbol: &Symbol,
    ty: &Ty,
    env: &Env,
    following: &mut Vec<Symbol>,
) -> Result<(), Error> {
    match ty {
        Ty::Named(Reference::Definition(name), _) => {
            if following.contains(name) {
                return Err(Error::Uncontractive(symbol.clone()));
            }
            let Some(info) = env.types.get(name) else {
                return Ok(());
            };
            following.push(name.clone());
            let result = contractive(symbol, &info.ty, env, following);
            following.pop();
            result
        }
        Ty::Union(members) => members
            .iter()
            .try_for_each(|member| contractive(symbol, member, env, following)),
        _ => Ok(()),
    }
}
