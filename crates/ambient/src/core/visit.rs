//! Walking core trees: what is inside an expression, and where each
//! expression is.

use super::{Expr, Key, Pattern, Primitive, Slot, Type};

/// The expressions directly inside one, in the order they are encoded,
/// for a walk that only looks and for one that changes them: written
/// once, so the two agree.
macro_rules! children {
    ($self:expr, $visit:ident, $iter:ident $(, $mutable:tt)?) => {
        match $self {
            Self::Null
            | Self::Literal(_)
            | Self::Local(_)
            | Self::Captured(_)
            | Self::Reference(..)
            | Self::AbilityId(_) => {}
            Self::Call { callee, args } => {
                $visit(callee);
                args.$iter().for_each($visit);
            }
            Self::Send { target, args, .. } => {
                $visit(target);
                args.$iter().for_each($visit);
            }
            Self::Record(fields) => fields.$iter().for_each(|(_, value)| $visit(value)),
            Self::Tuple(exprs)
            | Self::List(exprs)
            | Self::All(exprs)
            | Self::First(exprs)
            | Self::Primitive { args: exprs, .. }
            | Self::Perform { args: exprs, .. }
            | Self::Closure {
                captures: exprs, ..
            } => exprs.$iter().for_each($visit),
            Self::Field { target: inner, .. }
            | Self::Instantiate {
                function: inner, ..
            }
            | Self::Method(inner)
            | Self::Step(inner)
            | Self::Cell { value: inner, .. }
            | Self::Widen { value: inner, .. }
            | Self::Boundary(inner)
            | Self::Group(inner)
            | Self::Spawn(inner)
            | Self::At(_, inner) => $visit(inner),
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
                $visit(first);
                $visit(second);
            }
            Self::Match { subject, arms } => {
                $visit(subject);
                for (_, body) in arms {
                    $visit(body);
                }
            }
            Self::Budget {
                steps,
                allocation,
                body,
            } => {
                $visit(steps);
                $visit(allocation);
                $visit(body);
            }
            Self::Provide { handlers, body, .. } => {
                for handler in handlers {
                    $visit(&$($mutable)? handler.handler);
                }
                $visit(body);
            }
        }
    };
}

impl Expr {
    /// Whether a task may suspend somewhere in this: it runs tasks,
    /// waits on a cell, or reads its own trace, which is read off a
    /// suspension. See `spec/tasks.md`.
    #[must_use]
    pub fn suspends(&self) -> bool {
        let here = match self {
            Self::Group(_) | Self::Spawn(_) | Self::All(_) | Self::First(_) => true,
            Self::Primitive { operation, .. } => matches!(
                operation,
                Primitive::CellWait
                    | Primitive::Capture
                    | Primitive::ListAll
                    | Primitive::ListFirst
            ),
            _ => false,
        };
        let mut inside = false;
        self.children(&mut |child| inside = inside || child.suspends());
        here || inside
    }

    /// Calls `visit` with each expression directly inside this one, in
    /// the order they are encoded.
    pub fn children<'a>(&'a self, visit: &mut impl FnMut(&'a Self)) {
        children!(self, visit, iter);
    }

    /// [`Self::children`], for changing them.
    pub fn children_mut(&mut self, visit: &mut impl FnMut(&mut Self)) {
        children!(self, visit, iter_mut, mut);
    }

    /// Takes every mark out of this tree, and gives what each said:
    /// the surface node each expression was checked from, by
    /// [position](Self::positions). One no mark is around is from the
    /// node whatever holds it is from, and `from` at the top.
    pub fn origins(&mut self, from: u32) -> Vec<u32> {
        fn lift(expr: &mut Expr, mut from: u32, origins: &mut Vec<u32>) {
            while let Expr::At(marked, inner) = expr {
                from = *marked;
                *expr = std::mem::replace(&mut **inner, Expr::Null);
            }
            origins.push(from);
            expr.children_mut(&mut |child| lift(child, from, origins));
        }
        let mut origins = Vec::new();
        lift(self, from, &mut origins);
        origins
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

/// What a function's body does with the function's type parameters as
/// it runs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Consults {
    /// Whether it tests a value against a type that mentions one, or
    /// makes a cell at such a type.
    pub own: bool,
    /// The slots of the generic functions it uses at type arguments
    /// that mention one. It hands them on, if those functions need them.
    pub through: Vec<Slot>,
}

impl Pattern {
    /// Whether the pattern tests for a type that mentions a type
    /// parameter.
    fn mentions_parameter(&self) -> bool {
        match self {
            Self::Wildcard | Self::Literal(_) => false,
            Self::Record(fields) => fields.iter().any(|(_, inner)| inner.mentions_parameter()),
            Self::Tuple(members) => members.iter().any(Self::mentions_parameter),
            Self::Is(ty) => ty.mentions_parameter(),
            Self::Bind(inner) => inner.mentions_parameter(),
        }
    }
}

impl Expr {
    /// What this, a function's body, does with the function's type
    /// parameters. A function needs its type arguments as it runs only
    /// for one of these. See "Types at run time" in `spec/backends.md`.
    #[must_use]
    pub fn consults(&self) -> Consults {
        let mut found = Consults::default();
        for expr in self.positions() {
            match expr {
                Self::Cast { ty, .. } | Self::Cell { ty, .. } => {
                    found.own |= ty.mentions_parameter();
                }
                Self::Match { arms, .. } => {
                    found.own |= arms.iter().any(|(pattern, _)| pattern.mentions_parameter());
                }
                // Nothing says whether the function needs them.
                Self::Instantiate { types, .. }
                | Self::Send { types, .. }
                | Self::Perform { types, .. } => {
                    found.own |= types.iter().any(Type::mentions_parameter);
                }
                Self::Reference(slot, types)
                | Self::Closure {
                    slot,
                    type_args: types,
                    ..
                } if types.iter().any(Type::mentions_parameter) => {
                    found.through.push(*slot);
                }
                _ => {}
            }
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
            Self::List(member) | Self::Cell(member) | Self::Iterator(member) => {
                member.mentions_parameter()
            }
            Self::Function { params, ret, .. } => any(params) || ret.mentions_parameter(),
            Self::Generic { bounds, function } => any(bounds) || function.mentions_parameter(),
            _ => false,
        }
    }

    /// The type as the inside of a generic function type that binds
    /// the type parameters of the definition it is written in, `depth`
    /// parameters of generic function types being bound between.
    #[must_use]
    pub fn generalized(&self, depth: usize) -> Self {
        let all = |types: &[Self], depth: usize| -> Vec<Self> {
            types.iter().map(|ty| ty.generalized(depth)).collect()
        };
        let one = |ty: &Self, depth: usize| Box::new(ty.generalized(depth));
        match self {
            Self::Parameter(position) => Self::Bound(depth + position),
            Self::Record(fields) => {
                let field = |(key, ty): &(Key, Self)| (key.clone(), ty.generalized(depth));
                Self::Record(fields.iter().map(field).collect())
            }
            Self::Tuple(members) => Self::Tuple(all(members, depth)),
            Self::Union(members) => Self::Union(all(members, depth)),
            Self::Reference(slot, args) => Self::Reference(*slot, all(args, depth)),
            Self::List(member) => Self::List(one(member, depth)),
            Self::Cell(member) => Self::Cell(one(member, depth)),
            Self::Iterator(member) => Self::Iterator(one(member, depth)),
            Self::Method { needs, function } => Self::Method {
                needs: one(needs, depth),
                function: one(function, depth),
            },
            Self::Function { params, ret, row } => Self::Function {
                params: all(params, depth),
                ret: one(ret, depth),
                row: row.clone(),
            },
            Self::Generic { bounds, function } => Self::Generic {
                bounds: all(bounds, depth + bounds.len()),
                function: one(function, depth + bounds.len()),
            },
            _ => self.clone(),
        }
    }

    /// Every slot the type names, in the order it names them.
    #[must_use]
    pub fn slots(&self) -> Vec<Slot> {
        let all = |types: &[Self]| types.iter().flat_map(Self::slots).collect::<Vec<_>>();
        match self {
            Self::Record(fields) => fields
                .iter()
                .flat_map(|(key, ty)| {
                    let key = match key {
                        Key::Ability(slot) => Some(*slot),
                        Key::Field(_) => None,
                    };
                    key.into_iter().chain(ty.slots())
                })
                .collect(),
            Self::Tuple(members) | Self::Union(members) => all(members),
            Self::Reference(slot, args) => [*slot].into_iter().chain(all(args)).collect(),
            Self::List(member) | Self::Cell(member) | Self::Iterator(member) => member.slots(),
            Self::Function { params, ret, row } => {
                let mut slots = all(params);
                slots.extend(ret.slots());
                slots.extend(row);
                slots
            }
            Self::Generic { bounds, function } => {
                let mut slots = all(bounds);
                slots.extend(function.slots());
                slots
            }
            _ => Vec::new(),
        }
    }
}
