//! What a value can be tested for as a program runs, and the least a
//! test for a type asks. See "Narrowing" in `docs/spec/types.md`.

use super::{Env, Ty, TyKey, holds_match, mentions_parameter};
use crate::surface::Reference;
use crate::symbol::Symbol;

impl Env {
    /// Whether a normalized type holds, at any depth, an opaque type
    /// checking does not see into.
    #[must_use]
    pub fn holds_opaque(&self, ty: &Ty) -> bool {
        let held = std::cell::Cell::new(false);
        let _ = ty.rewrite(0, &|ty, _| {
            if self.revealed(ty).is_some() {
                held.set(true);
            }
            None
        });
        held.get()
    }

    /// Whether a value can be tested against a type as a program runs.
    /// A function says which type it is of and no more, so a function
    /// type in it must be one type whatever the type arguments in
    /// force: it may mention no type parameter. Inside a cell type
    /// anything may be, since a cell keeps the type it was made at.
    #[must_use]
    pub fn testable(&self, ty: &Ty) -> bool {
        self.tests(ty, &mut Vec::new())
    }

    /// [`Self::testable`], given the definitions already looked into.
    fn tests<'a>(&'a self, ty: &'a Ty, seen: &mut Vec<&'a Symbol>) -> bool {
        match ty {
            // An iterator that a function steps is tested as that
            // function is.
            Ty::Function { .. } | Ty::Generic { .. } | Ty::Iterator(_) => !mentions_parameter(ty),
            // A test never yields a method: see `docs/spec/methods.md`. And
            // nothing of a brand on a type is there as a program runs, nor
            // anything that decides a type match.
            Ty::Method { .. } | Ty::Branded(..) | Ty::Match(_) | Ty::Matched(_) => false,
            Ty::Cell(member) => !holds_match(member),
            Ty::Record(fields) => fields.iter().all(|(_, ty)| self.tests(ty, seen)),
            Ty::Tuple(members) => members.iter().all(|member| self.tests(member, seen)),
            Ty::Union(members) => members.iter().all(|member| self.tests(member, seen)),
            Ty::List(member) => self.tests(member, seen),
            // An opaque type checking does not see into is not there as a
            // program runs, as a brand is not: only its content is.
            Ty::Named(Reference::Definition(symbol), _) if !self.sees(symbol) => false,
            Ty::Named(name, args) => {
                let definition = match name {
                    Reference::Definition(symbol) if !seen.contains(&symbol) => {
                        seen.push(symbol);
                        self.types.get(symbol)
                    }
                    _ => None,
                };
                args.iter().all(|arg| self.tests(arg, seen))
                    && definition.is_none_or(|info| self.tests(&info.ty, seen))
            }
            _ => true,
        }
    }

    /// The nearest type to `ty` that a value can be tested for: `ty`
    /// itself if it can be, and otherwise `ty` without its methods and
    /// with unknown for each part no test can tell. A test for it lets
    /// through every value of `ty`, and perhaps others.
    #[must_use]
    pub fn nearest_test(&self, ty: &Ty) -> Ty {
        if self.testable(ty) {
            return ty.clone();
        }
        let part = |ty: &Ty| {
            if self.testable(ty) {
                ty.clone()
            } else {
                Ty::Unknown
            }
        };
        // What is under an opaque type is all there is to test.
        if let Some(content) = self.revealed(&self.unfold(ty)) {
            return self.nearest_test(&content);
        }
        match self.unfold(ty) {
            Ty::Record(fields) => {
                let held = fields
                    .iter()
                    .filter(|(_, ty)| !matches!(ty, Ty::Method { .. }));
                Ty::Record(held.map(|(key, ty)| (key.clone(), part(ty))).collect())
            }
            Ty::Tuple(members) => Ty::Tuple(members.iter().map(part).collect()),
            Ty::List(member) => Ty::List(Box::new(part(&member))),
            // What is under a brand is all there is to test.
            Ty::Branded(ty, _) => self.nearest_test(&ty),
            Ty::Union(members) => self.union(
                members
                    .iter()
                    .map(|member| self.nearest_test(member))
                    .collect(),
            ),
            _ => Ty::Unknown,
        }
    }

    /// The least a test for `ty` can ask: the shape of a value of it,
    /// and nothing of what the value holds. A function is asked only
    /// to be one, and an iterator only to be one. It lets through
    /// every value of `ty`, and whatever else has the shape.
    #[must_use]
    pub fn shape_test(&self, ty: &Ty) -> Ty {
        let unknown = |ty: &Ty| match ty {
            Ty::Method { .. } => None,
            _ => Some(Ty::Unknown),
        };
        if let Some(content) = self.revealed(&self.unfold(ty)) {
            return self.shape_test(&content);
        }
        match self.unfold(ty) {
            Ty::Record(fields) => {
                let held = fields.iter();
                let held = held.filter_map(|(key, ty)| Some((key.clone(), unknown(ty)?)));
                Ty::Record(held.collect())
            }
            Ty::Tuple(members) => Ty::Tuple(members.each(|_| Ty::Unknown)),
            Ty::List(_) => Ty::List(Box::new(Ty::Unknown)),
            Ty::Iterator(_) => Ty::Iterator(Box::new(Ty::Unknown)),
            Ty::Function { .. } | Ty::Generic { .. } => Ty::Callable,
            Ty::Branded(ty, _) => self.shape_test(&ty),
            Ty::Union(members) => self.union(
                members
                    .iter()
                    .map(|member| self.shape_test(member))
                    .collect(),
            ),
            other => self.nearest_test(&other),
        }
    }

    /// A test for `ty` that asks of each part what [`Self::nearest_test`]
    /// does, but of a function only that it is one, and of an iterator
    /// only that it is one. A type definition that refers to itself is
    /// left as it is.
    #[must_use]
    pub fn loose_test(&self, ty: &Ty) -> Ty {
        let each = |types: &[Ty]| types.iter().map(|ty| self.loose_test(ty)).collect();
        match &self.nearest_test(ty) {
            Ty::Record(fields) => {
                let loose = |(key, ty): &(TyKey, Ty)| (key.clone(), self.loose_test(ty));
                Ty::Record(fields.iter().map(loose).collect())
            }
            Ty::Tuple(members) => Ty::Tuple(members.each(|member| self.loose_test(member))),
            Ty::List(member) => Ty::List(Box::new(self.loose_test(member))),
            Ty::Iterator(_) => Ty::Iterator(Box::new(Ty::Unknown)),
            Ty::Function { .. } | Ty::Generic { .. } => Ty::Callable,
            Ty::Union(members) => self.union(each(members)),
            near => near.clone(),
        }
    }

    /// Whether a test for `ty` asks a function, or an iterator a
    /// function steps, for its type. Such a value says the one type it
    /// was written as, which may be within the one asked for, so the
    /// test may turn away a value of `ty`.
    #[must_use]
    pub fn asks_a_name(&self, ty: &Ty) -> bool {
        self.names(ty, &mut Vec::new())
    }

    /// [`Self::asks_a_name`], given the definitions already looked into.
    fn names<'a>(&'a self, ty: &'a Ty, seen: &mut Vec<&'a Symbol>) -> bool {
        match ty {
            Ty::Function { .. } | Ty::Generic { .. } => true,
            Ty::Iterator(item) => **item != Ty::Unknown,
            Ty::Branded(ty, _) | Ty::List(ty) => self.names(ty, seen),
            Ty::Record(fields) => fields.iter().any(|(_, ty)| self.names(ty, seen)),
            Ty::Tuple(members) => members.iter().any(|member| self.names(member, seen)),
            Ty::Union(members) => members.iter().any(|member| self.names(member, seen)),
            Ty::Named(name, args) => {
                let definition = match name {
                    Reference::Definition(symbol) if !seen.contains(&symbol) => {
                        seen.push(symbol);
                        self.types.get(symbol)
                    }
                    _ => None,
                };
                args.iter().any(|arg| self.names(arg, seen))
                    || definition.is_some_and(|info| self.names(&info.ty, seen))
            }
            _ => false,
        }
    }
}
