//! What each operation takes and gives. See `spec/operations.md`.
//!
//! Both checkers ask here: the checker to find which operation an
//! operator means at its operands' types, and the core checker to see
//! that a tree applies an operation to what it takes.

use super::{Env, Ty};
use crate::core::Primitive;

impl Env {
    /// The type of the members of a list: what `ty` is a list of, if
    /// every value of it is a list.
    #[must_use]
    pub fn members(&self, ty: &Ty) -> Option<Ty> {
        match self.unfold(ty) {
            Ty::List(member) => Some(*member),
            Ty::Union(alternatives) => {
                let members: Option<Vec<Ty>> =
                    alternatives.iter().map(|ty| self.members(ty)).collect();
                Some(self.union(members?))
            }
            Ty::Never => Some(Ty::Never),
            _ => None,
        }
    }

    /// What a step of an iterator of `item` gives: nothing, or an item
    /// and the iterator of the rest.
    #[must_use]
    pub fn step(&self, item: &Ty) -> Ty {
        let rest = Ty::Iterator(Box::new(item.clone()));
        self.union(vec![Ty::Null, Ty::Tuple(vec![item.clone(), rest])])
    }

    /// The type of the items of an iterator: what `ty` is an iterator
    /// of, if every value of it is one.
    #[must_use]
    pub fn items(&self, ty: &Ty) -> Option<Ty> {
        match self.unfold(ty) {
            Ty::Iterator(item) => Some(*item),
            Ty::Union(alternatives) => {
                let items: Option<Vec<Ty>> = alternatives.iter().map(|ty| self.items(ty)).collect();
                Some(self.union(items?))
            }
            Ty::Never => Some(Ty::Never),
            _ => None,
        }
    }

    /// The type of the items a function gives if it steps an iterator:
    /// it takes nothing from its caller, and gives a step of them.
    fn stepped_by(&self, function: &Ty) -> Option<Ty> {
        let Ty::Function { params, ret, row } = self.unfold(function) else {
            return None;
        };
        let steps = match self.unfold(&ret) {
            Ty::Union(steps) => steps,
            step => vec![step],
        };
        // Whatever it gives beside null is an item and the rest, and
        // the rest gives items too.
        let mut items = Vec::new();
        for step in &steps {
            if let Ty::Tuple(members) = self.unfold(step)
                && let [item, rest] = &members[..]
            {
                items.push(item.clone());
                items.push(self.items(rest)?);
            }
        }
        let item = self.union(items);
        let fits = params.is_empty() && row.is_empty() && self.subtype(&ret, &self.step(&item));
        fits.then_some(item)
    }

    /// The type of what an operation gives for operands of these types,
    /// all normalized, or nothing if it does not take them.
    ///
    /// Operations on cells and the one that captures a trace are not
    /// here: they are typed where they are checked.
    #[must_use]
    pub fn operation(&self, operation: Primitive, operands: &[Ty]) -> Option<Ty> {
        use Primitive as P;
        let all = |ty: &Ty| operands.iter().all(|operand| self.subtype(operand, ty));
        let is = |operand: &Ty, ty: &Ty| self.subtype(operand, ty);
        let list = |members: Vec<Ty>| Ty::List(Box::new(self.union(members)));
        let or_null = |ty: Ty| self.union(vec![ty, Ty::Null]);
        Some(match (operation, operands) {
            (P::Equal, [_, _]) => Ty::Bool,
            (
                P::IntAdd | P::IntSubtract | P::IntMultiply | P::IntDivide | P::IntRemainder,
                [_, _],
            ) if all(&Ty::Int) => Ty::Int,
            (P::FloatAdd | P::FloatSubtract | P::FloatMultiply | P::FloatDivide, [_, _])
                if all(&Ty::Float) =>
            {
                Ty::Float
            }
            (P::IntLess, [_, _]) if all(&Ty::Int) => Ty::Bool,
            (P::FloatLess, [_, _]) if all(&Ty::Float) => Ty::Bool,
            (P::StringLess, [_, _]) if all(&Ty::String) => Ty::Bool,
            (P::StringLength, [_]) if all(&Ty::String) => Ty::Int,
            (P::StringJoin, [_, _]) if all(&Ty::String) => Ty::String,
            (P::StringSlice, [text, start, end])
                if is(text, &Ty::String) && is(start, &Ty::Int) && is(end, &Ty::Int) =>
            {
                or_null(Ty::String)
            }
            (P::ListLength, [values]) => self.members(values).map(|_| Ty::Int)?,
            (P::ListGet, [values, position]) if is(position, &Ty::Int) => {
                or_null(self.members(values)?)
            }
            (P::ListPush, [values, member]) => list(vec![self.members(values)?, member.clone()]),
            (P::ListJoin, [left, right]) => list(vec![self.members(left)?, self.members(right)?]),
            (P::ListSlice, [values, start, end]) if is(start, &Ty::Int) && is(end, &Ty::Int) => {
                or_null(list(vec![self.members(values)?]))
            }
            (P::IntFloat, [_]) if all(&Ty::Int) => Ty::Float,
            (P::FloatInt, [_]) if all(&Ty::Float) => or_null(Ty::Int),
            (P::IntText, [_]) if all(&Ty::Int) => Ty::String,
            (P::FloatText, [_]) if all(&Ty::Float) => Ty::String,
            (P::StringInt, [_]) if all(&Ty::String) => or_null(Ty::Int),
            (P::StringFloat, [_]) if all(&Ty::String) => or_null(Ty::Float),
            (P::IteratorNew, [function]) => Ty::Iterator(Box::new(self.stepped_by(function)?)),
            (P::ListIterate, [values]) => Ty::Iterator(Box::new(self.members(values)?)),
            (P::StringIterate, [_]) if all(&Ty::String) => Ty::Iterator(Box::new(Ty::String)),
            (P::IntRange, [_, _]) if all(&Ty::Int) => Ty::Iterator(Box::new(Ty::Int)),
            _ => return None,
        })
    }
}
