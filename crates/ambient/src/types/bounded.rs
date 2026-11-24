//! Whether a type applies each type definition only to arguments within
//! the bounds of its parameters. See "Generics" in `docs/spec/types.md`.

use super::{Env, Ty, instantiate, substitute};
use crate::surface::Reference;

/// An argument a type definition was applied to outside the bound of
/// its parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unbounded {
    /// The bound, with the definition's arguments in place of its
    /// parameters.
    pub bound: Ty,
    pub found: Ty,
}

impl Env {
    /// The first argument in `ty` that a type definition is applied to
    /// outside the bound of its parameter, if there is one.
    ///
    /// A parameter of a generic function type in `ty` is known by its
    /// bound, as one of a definition is, and a binder of a type match's
    /// arm by what the arm's pattern narrows the subject to.
    #[must_use]
    pub fn unbounded(&self, ty: &Ty) -> Option<Unbounded> {
        let first = |types: &[Ty]| types.iter().find_map(|ty| self.unbounded(ty));
        match ty {
            Ty::Named(Reference::Definition(symbol), args) => {
                let applied = self.types.get(symbol).and_then(|info| {
                    let bounds = info.bounds.iter();
                    bounds.zip(args).find_map(|(bound, arg)| {
                        let bound = self.normalize(&substitute(bound, args));
                        // Types are related in normal form.
                        let within = self.subtype(&self.normalize(arg), &bound);
                        (!within).then(|| Unbounded {
                            bound,
                            found: arg.clone(),
                        })
                    })
                });
                applied.or_else(|| first(args))
            }
            Ty::Named(_, args) => first(args),
            Ty::Record(fields) => fields.iter().find_map(|(_, ty)| self.unbounded(ty)),
            Ty::Tuple(members) => first(members),
            Ty::Union(members) => first(members),
            Ty::List(member) | Ty::Cell(member) | Ty::Iterator(member) => self.unbounded(member),
            Ty::Branded(under, brands) => self.unbounded(under).or_else(|| first(brands)),
            Ty::Method { needs, function } => {
                self.unbounded(needs).or_else(|| self.unbounded(function))
            }
            Ty::Function { params, ret, .. } => first(params).or_else(|| self.unbounded(ret)),
            Ty::Generic { bounds, function } => self.unbounded_generic(bounds, function),
            Ty::Match(matching) => {
                let tested = matching
                    .arms
                    .iter()
                    .flat_map(|(pattern, _)| pattern.tested());
                let arms = matching
                    .arms
                    .iter()
                    .map(|(pattern, ty)| self.arm_known(&matching.subject, pattern, ty));
                self.unbounded(&matching.subject)
                    .or_else(|| tested.into_iter().find_map(|ty| self.unbounded(ty)))
                    .or_else(|| arms.into_iter().find_map(|ty| self.unbounded(&ty)))
            }
            _ => None,
        }
    }

    /// [`Self::unbounded`] of a generic function type, with its
    /// parameters in scope at their bounds.
    fn unbounded_generic(&self, bounds: &[Ty], function: &Ty) -> Option<Unbounded> {
        let base = self.parameters();
        let fresh: Vec<Ty> = (base..base + bounds.len())
            .map(|position| Ty::Named(Reference::Parameter(position), Vec::new()))
            .collect();
        let opened: Vec<Ty> = bounds
            .iter()
            .map(|bound| instantiate(bound, &fresh))
            .collect();
        self.inner.borrow_mut().extend(opened.iter().cloned());
        let found = opened
            .iter()
            .find_map(|bound| self.unbounded(bound))
            .or_else(|| self.unbounded(&instantiate(function, &fresh)));
        self.inner.borrow_mut().truncate(base - self.bounds.len());
        found
    }
}
