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
            Self::List(member) => member.names(visit),
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
    /// Calls `visit` with every name in the expression, in order.
    pub fn names<'a>(&'a self, visit: &mut impl FnMut(&'a N)) {
        match self {
            Self::Null | Self::Literal(_) => {}
            Self::Name(name) => visit(name),
            Self::Instantiate { function, args } => {
                visit(function);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Call { callee, args } => {
                callee.names(visit);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Record(fields) => {
                for (key, value) in fields {
                    key.names(visit);
                    value.names(visit);
                }
            }
            Self::Tuple(members) | Self::List(members) => {
                for member in members {
                    member.names(visit);
                }
            }
            Self::Field { target, key } => {
                target.names(visit);
                key.names(visit);
            }
            Self::Let { value, body, .. } => {
                value.names(visit);
                body.names(visit);
            }
            Self::Binary { left, right, .. } | Self::Merge { left, right } => {
                left.names(visit);
                right.names(visit);
            }
            Self::Match { subject, arms } => {
                subject.names(visit);
                for arm in arms {
                    arm.pattern.names(visit);
                    arm.body.names(visit);
                }
            }
            Self::Lambda { params, row, body } => {
                for ty in params.iter().filter_map(|(_, ty)| ty.as_ref()) {
                    ty.names(visit);
                }
                row.iter().for_each(&mut *visit);
                body.names(visit);
            }
            Self::Perform { ability, args } => {
                visit(ability);
                for arg in args {
                    arg.names(visit);
                }
            }
            Self::Provide { bundle, body, .. } => {
                bundle.names(visit);
                body.names(visit);
            }
            Self::Comptime(call) => call.names(visit),
            Self::Cast {
                value,
                ty,
                otherwise,
            } => {
                value.names(visit);
                ty.names(visit);
                otherwise.names(visit);
            }
        }
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
        let all = |exprs: &[Self]| exprs.iter().map(Self::binders).sum::<usize>();
        match self {
            Self::Null | Self::Literal(_) | Self::Name(_) | Self::Instantiate { .. } => 0,
            Self::Call { callee, args } => callee.binders() + all(args),
            Self::Record(fields) => fields.iter().map(|(_, value)| value.binders()).sum(),
            Self::Tuple(members) | Self::List(members) => all(members),
            Self::Field { target, .. } => target.binders(),
            Self::Let { value, body, .. } => 1 + value.binders() + body.binders(),
            Self::Binary { left, right, .. } | Self::Merge { left, right } => {
                left.binders() + right.binders()
            }
            Self::Match { subject, arms } => {
                let arms: usize = arms
                    .iter()
                    .map(|arm| arm.pattern.binders() + arm.body.binders())
                    .sum();
                subject.binders() + arms
            }
            Self::Lambda { params, body, .. } => params.len() + body.binders(),
            Self::Perform { args, .. } => all(args),
            Self::Provide { bundle, body, .. } => bundle.binders() + 1 + body.binders(),
            Self::Comptime(call) => call.binders(),
            Self::Cast {
                value, otherwise, ..
            } => value.binders() + otherwise.binders(),
        }
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

    /// Calls `visit` with each expression directly inside this one.
    fn children<'a>(&'a self, visit: &mut impl FnMut(&'a Self)) {
        match self {
            Self::Null | Self::Literal(_) | Self::Name(_) | Self::Instantiate { .. } => {}
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
            Self::Comptime(call) => visit(call),
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
