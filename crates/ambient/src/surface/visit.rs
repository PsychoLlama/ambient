//! Walking surface trees: the names in one, and how many binders it has.

use super::{Definition, DefinitionKind, Expr, ExternKind, Function, Key, Pattern, Type};

impl<N> Key<N> {
    fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        if let Self::Ability(ability) = self {
            visit(ability);
        }
    }
}

impl<N> Type<N> {
    /// Calls `visit` with every name in the type, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Never
            | Self::Unknown
            | Self::Null
            | Self::Bool
            | Self::Int
            | Self::Float
            | Self::String
            | Self::Tag
            | Self::Literal(_) => {}
            Self::Record(fields) => {
                for (key, ty) in fields {
                    key.names(visit);
                    ty.names(visit);
                }
            }
            Self::Tuple(members) | Self::Union(members) => {
                for ty in members {
                    ty.names(visit);
                }
            }
            Self::List(member) | Self::Cell(member) => member.names(visit),
            Self::Function { params, ret, row } => {
                for ty in params {
                    ty.names(visit);
                }
                ret.names(visit);
                row.iter().for_each(&mut *visit);
            }
            Self::Named(name, args) => {
                visit(name);
                for arg in args {
                    arg.names(visit);
                }
            }
        }
    }
}

impl<N> Pattern<N> {
    /// Calls `visit` with every name in the pattern's types, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Wildcard | Self::Literal(_) => {}
            Self::Record(fields) => fields.iter().for_each(|(_, p)| p.names(visit)),
            Self::Tuple(members) => members.iter().for_each(|p| p.names(visit)),
            Self::Is(ty) => ty.names(visit),
            Self::Bind(_, inner) => inner.names(visit),
        }
    }
}

impl<N> Expr<N> {
    /// Calls `visit` with every name in the expression: those a node
    /// holds itself, then those of the expressions inside it.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Name(name) | Self::AbilityId(name) | Self::Perform { ability: name, .. } => {
                visit(name);
            }
            Self::Instantiate { function, args } => {
                visit(function);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Record(fields) => fields.iter().for_each(|(key, _)| key.names(visit)),
            Self::Field { key, .. } => key.names(visit),
            Self::Match { arms, .. } => arms.iter().for_each(|arm| arm.pattern.names(visit)),
            Self::Lambda { params, row, .. } => {
                for ty in params.iter().filter_map(|(_, ty)| ty.as_ref()) {
                    ty.names(visit);
                }
                row.iter().for_each(&mut *visit);
            }
            Self::Cell { ty: Some(ty), .. } | Self::Cast { ty, .. } => ty.names(visit),
            _ => {}
        }
        self.children(&mut |child| child.names(visit));
    }
}

impl<N> Pattern<N> {
    /// How many names the pattern binds.
    #[must_use]
    pub fn binders(&self) -> usize {
        match self {
            Self::Wildcard | Self::Literal(_) | Self::Is(_) => 0,
            Self::Record(fields) => fields.iter().map(|(_, p)| p.binders()).sum(),
            Self::Tuple(members) => members.iter().map(Self::binders).sum(),
            Self::Bind(_, inner) => 1 + inner.binders(),
        }
    }
}

impl<N> Expr<N> {
    /// How many names are bound anywhere inside the expression. Locals are
    /// numbered in binding order, so this is how many numbers it uses.
    #[must_use]
    pub fn binders(&self) -> usize {
        let own = match self {
            Self::Let { .. } | Self::Provide { .. } | Self::Observe { .. } => 1,
            Self::Lambda { params, .. } => params.len(),
            Self::Match { arms, .. } => arms.iter().map(|arm| arm.pattern.binders()).sum(),
            _ => 0,
        };
        let mut inside = 0;
        self.children(&mut |child| inside += child.binders());
        own + inside
    }
}

impl<N> Expr<N> {
    /// Calls `visit` with the callee of every comptime call inside the
    /// expression.
    pub fn comptime_callees<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        if let Self::Comptime(call) = self
            && let Self::Call { callee, .. } = &**call
            && let Self::Name(name) = &**callee
        {
            visit(name);
        }
        self.children(&mut |child| child.comptime_callees(visit));
    }

    /// Calls `visit` with each expression directly inside this one, in
    /// order. A walk that treats most nodes alike is written with this,
    /// so that a new node is added here and not to every walk.
    pub fn children<'a>(&'a self, visit: &mut impl FnMut(&'a Self)) {
        match self {
            Self::Null
            | Self::Trace
            | Self::Literal(_)
            | Self::Name(_)
            | Self::Instantiate { .. }
            | Self::AbilityId(_) => {}
            Self::Call { callee, args } => {
                visit(callee);
                args.iter().for_each(visit);
            }
            Self::Record(fields) => fields.iter().for_each(|(_, value)| visit(value)),
            Self::Tuple(members) | Self::List(members) => members.iter().for_each(visit),
            Self::Field { target, .. } => visit(target),
            Self::Let { value, body, .. } => {
                visit(value);
                visit(body);
            }
            Self::Binary { left, right, .. } | Self::Merge { left, right } => {
                visit(left);
                visit(right);
            }
            Self::Match { subject, arms } => {
                visit(subject);
                for arm in arms {
                    visit(&arm.body);
                }
            }
            Self::Lambda { body, .. } => visit(body),
            Self::Perform { args, .. } => args.iter().for_each(visit),
            Self::Provide { bundle, body, .. } => {
                visit(bundle);
                visit(body);
            }
            Self::Observe { observer, body }
            | Self::Cleanup {
                cleanup: observer,
                body,
            } => {
                visit(observer);
                visit(body);
            }
            Self::All(exprs) | Self::First(exprs) | Self::Cell { args: exprs, .. } => {
                exprs.iter().for_each(visit);
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
            Self::Comptime(call) | Self::Group(call) | Self::Spawn(call) | Self::Boundary(call) => {
                visit(call);
            }
            Self::Cast {
                value, otherwise, ..
            } => {
                visit(value);
                visit(otherwise);
            }
        }
    }
}

impl<N> Function<N> {
    /// Calls `visit` with every name in the header, in order.
    pub fn header_names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        for bound in self.type_params.iter().filter_map(|p| p.bound.as_ref()) {
            bound.names(visit);
        }
        self.params.iter().for_each(|param| param.ty.names(visit));
        self.ret.names(visit);
        self.row.iter().for_each(visit);
    }
}

impl<N> Definition<N> {
    /// Calls `visit` with every name in the definition, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match &self.kind {
            DefinitionKind::Type(definition) => definition.ty.names(visit),
            DefinitionKind::Function(function) => {
                function.header_names(visit);
                function.body.names(visit);
            }
            DefinitionKind::Ability(ability) => {
                ability
                    .params
                    .iter()
                    .for_each(|param| param.ty.names(visit));
                ability.ret.names(visit);
            }
            DefinitionKind::Extern(native) => match &native.kind {
                ExternKind::Function { params, ret } => {
                    for param in params {
                        param.ty.names(visit);
                    }
                    ret.names(visit);
                }
                ExternKind::Handler { ability, row } => {
                    visit(ability);
                    row.iter().for_each(visit);
                }
            },
            DefinitionKind::Target(target) => {
                target.extends.iter().for_each(&mut *visit);
                for (ability, handler) in &target.bindings {
                    visit(ability);
                    visit(handler);
                }
            }
        }
    }
}
