//! Generic function types: putting types in place of the parameters
//! one binds, and binding the parameters of a definition. See
//! "Generics" in `spec/types.md`.

use super::Ty;
use crate::surface::Reference;

/// `ty`, which is inside a generic function type that binds as many
/// parameters as there are `args`, with each of those replaced by the
/// argument in its position. The arguments mention no parameter bound
/// outside themselves.
#[must_use]
pub fn instantiate(ty: &Ty, args: &[Ty]) -> Ty {
    ty.rewrite(0, &|ty, depth| {
        let position = ty.bound_at(depth, args.len())?;
        Some(args[position].clone())
    })
}

/// `ty` as the inside of a generic function type that binds, in order,
/// the `count` type parameters of the definition being checked from
/// `from` on.
#[must_use]
pub fn generalize(ty: &Ty, from: usize, count: usize) -> Ty {
    ty.rewrite(0, &|ty, depth| match ty {
        Ty::Named(Reference::Parameter(position), _) if (from..from + count).contains(position) => {
            Some(Ty::Bound(depth + position - from))
        }
        _ => None,
    })
}
