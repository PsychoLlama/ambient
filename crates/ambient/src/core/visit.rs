//! Walking core trees: what is inside an expression, and where each
//! expression is.

use super::{Expr, Slot, Type};

impl Expr {
    /// Calls `visit` with each expression directly inside this one, in
    /// the order they are encoded.
    pub fn children<'a>(&'a self, visit: &mut impl FnMut(&'a Self)) {
        match self {
            Self::Null
            | Self::Literal(_)
            | Self::Local(_)
            | Self::Captured(_)
            | Self::Reference(..)
            | Self::AbilityId(_) => {}
            Self::Call { callee, args } => {
                visit(callee);
                args.iter().for_each(visit);
            }
            Self::Record(fields) => fields.iter().for_each(|(_, value)| visit(value)),
            Self::Tuple(exprs)
            | Self::List(exprs)
            | Self::All(exprs)
            | Self::First(exprs)
            | Self::Primitive { args: exprs, .. }
            | Self::Perform { args: exprs, .. }
            | Self::Closure {
                captures: exprs, ..
            } => exprs.iter().for_each(visit),
            Self::Field { target: inner, .. }
            | Self::Cell { value: inner, .. }
            | Self::Boundary(inner)
            | Self::Group(inner)
            | Self::Spawn(inner) => visit(inner),
            Self::Merge {
                left: first,
                right: second,
                ..
            }
            | Self::Let {
                value: first,
                body: second,
            }
            | Self::Cast {
                value: first,
                otherwise: second,
                ..
            }
            | Self::Cleanup {
                cleanup: first,
                body: second,
            } => {
                visit(first);
                visit(second);
            }
            Self::Match { subject, arms } => {
                visit(subject);
                for (_, body) in arms {
                    visit(body);
                }
            }
            Self::Budget {
                steps,
                allocation,
                body,
            } => {
                visit(steps);
                visit(allocation);
                visit(body);
            }
            Self::Provide { handlers, body, .. } => {
                for handler in handlers {
                    visit(&handler.handler);
                }
                visit(body);
            }
        }
    }

    /// Every expression in this one, each before what is inside it. An
    /// expression's place in this list is its position: what a frame of
    /// a trace names. See `spec/errors.md`.
    #[must_use]
    pub fn positions(&self) -> Vec<&Self> {
        let mut found = Vec::new();
        let mut pending = vec![self];
        while let Some(expr) = pending.pop() {
            found.push(expr);
            let from = pending.len();
            expr.children(&mut |child| pending.push(child));
            // The first child is to be taken first.
            pending[from..].reverse();
        }
        found
    }

    /// The slots of the functions this makes values of: every function
    /// it names, apart from where it only calls one.
    #[must_use]
    pub fn values(&self) -> Vec<Slot> {
        let mut called = Vec::new();
        let mut found = Vec::new();
        for expr in self.positions() {
            match expr {
                Self::Call { callee, .. } => called.push(std::ptr::from_ref(&**callee)),
                Self::Closure { slot, .. } => found.push(*slot),
                Self::Reference(slot, _) if !called.contains(&std::ptr::from_ref(expr)) => {
                    found.push(*slot);
                }
                _ => {}
            }
        }
        found
    }

    /// The slots of the functions named inside the budgets this enters,
    /// if it enters any.
    #[must_use]
    pub fn budgeted(&self) -> Option<Vec<Slot>> {
        let mut found = None;
        for expr in self.positions() {
            let Self::Budget { body, .. } = expr else {
                continue;
            };
            let named = body
                .positions()
                .into_iter()
                .filter_map(|inside| match inside {
                    Self::Reference(slot, _) | Self::Closure { slot, .. } => Some(*slot),
                    _ => None,
                });
            found.get_or_insert_with(Vec::new).extend(named);
        }
        found
    }
}

impl Type {
    /// Whether the type mentions a type parameter of the definition it
    /// is written in: whether what it describes depends on that
    /// definition's type arguments.
    #[must_use]
    pub fn mentions_parameter(&self) -> bool {
        let any = |types: &[Self]| types.iter().any(Self::mentions_parameter);
        match self {
            Self::Parameter(_) => true,
            Self::Record(fields) => fields.iter().any(|(_, ty)| ty.mentions_parameter()),
            Self::Tuple(members) | Self::Union(members) | Self::Reference(_, members) => {
                any(members)
            }
            Self::List(member) | Self::Cell(member) => member.mentions_parameter(),
            Self::Function { params, ret, .. } => any(params) || ret.mentions_parameter(),
            _ => false,
        }
    }
}
