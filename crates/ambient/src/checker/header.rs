//! Headers and type definitions: what is checked without looking at a
//! body.

use super::Checker;
use crate::core::{self, Open, Tree};
use crate::error::Error;
use crate::surface::{Ability, Function, Reference, TypeDefinition};
use crate::symbol::Symbol;
use crate::types::{Env, Ty, abilities};

/// Checks a type definition.
///
/// # Errors
///
/// Fails if the type is ill-formed: recursive without passing through a
/// constructor, or applying itself to anything but its own parameters.
pub fn check_type(
    symbol: &Symbol,
    definition: &TypeDefinition<Reference>,
    env: &Env,
) -> Result<Open, Error> {
    contractive(symbol, &definition.ty, env, &mut vec![symbol.clone()])?;
    let mut checker = Checker::new(symbol, env);
    checker.well_formed(&definition.ty)?;
    regular(symbol, &definition.ty, definition.params.len())?;
    let ty = checker.lower(&env.normalize(&definition.ty));
    Ok(Open {
        tree: Tree::Type {
            params: definition.params.len(),
            ty,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
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
    env: &Env,
) -> Result<Open, Error> {
    let mut checker = Checker::new(symbol, env);
    let mut params = Vec::new();
    for param in &ability.params {
        checker.well_formed(&param.ty)?;
        params.push(checker.lower(&env.normalize(&param.ty)));
    }
    checker.well_formed(&ability.ret)?;
    let ret = checker.lower(&env.normalize(&ability.ret));
    Ok(Open {
        tree: Tree::Ability {
            package: symbol.module.package.0.clone(),
            tag: ability.tag.clone(),
            params,
            ret,
        },
        slots: checker.finish().slots,
        lambdas: Vec::new(),
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

/// Fails unless every use of `symbol` inside its own definition applies it
/// to exactly its own parameters, in order.
fn regular(symbol: &Symbol, ty: &Ty, params: usize) -> Result<(), Error> {
    let mut result = Ok(());
    visit_types(ty, &mut |ty| {
        if let Ty::Named(Reference::Definition(name), args) = ty
            && name == symbol
        {
            let own = args.len() == params
                && args.iter().enumerate().all(|(position, arg)| {
                    matches!(arg, Ty::Named(Reference::Parameter(p), _) if *p == position)
                });
            if !own {
                result = Err(Error::IrregularType(symbol.clone()));
            }
        }
    });
    result
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
        Ty::List(member) => visit_types(member, visit),
        Ty::Function { params, ret, .. } => {
            for param in params {
                visit_types(param, visit);
            }
            visit_types(ret, visit);
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
