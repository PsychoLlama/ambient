//! Generic function types: putting types in place of the parameters
//! one binds, and binding the parameters of a definition. See
//! "Generics" in `spec/types.md`.

use super::{AbilityInfo, Env, Signature, Ty};
use crate::surface::Reference;

impl Env {
    /// How many type parameters are in scope.
    #[must_use]
    pub fn parameters(&self) -> usize {
        self.bounds.len() + self.inner.borrow().len()
    }

    /// The bound of the type parameter at `position`, if one is in
    /// scope there.
    #[must_use]
    pub fn bound(&self, position: usize) -> Option<Ty> {
        let inner = || {
            self.inner
                .borrow()
                .get(position - self.bounds.len())
                .cloned()
        };
        self.bounds.get(position).cloned().or_else(inner)
    }

    /// The bound of every type parameter in scope, in order.
    #[must_use]
    pub fn all_bounds(&self) -> Vec<Ty> {
        [self.bounds.clone(), self.inner.borrow().clone()].concat()
    }
}

impl AbilityInfo {
    /// The type of a handler for the ability that gives `ret` and takes
    /// `row` from its caller: a generic function type if the ability's
    /// signature has type parameters.
    #[must_use]
    pub fn handler(&self, ret: Ty, row: Vec<Reference>) -> Ty {
        let function = Ty::Function {
            params: self.params.clone(),
            ret: Box::new(ret),
            row,
        };
        let count = self.type_params.len();
        if count == 0 {
            return function;
        }
        let bounds = self.type_params.iter();
        Ty::Generic {
            bounds: bounds.map(|bound| generalize(bound, 0, count)).collect(),
            function: Box::new(generalize(&function, 0, count)),
        }
    }

    /// The ability as the header of a function a perform calls.
    #[must_use]
    pub fn signature(&self) -> Signature {
        Signature {
            type_params: self.type_params.clone(),
            params: self.params.clone(),
            ret: self.ret.clone(),
            row: Vec::new(),
            native: false,
        }
    }
}

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
