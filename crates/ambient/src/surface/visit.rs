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
            | Self::Bytes
            | Self::Tag
            | Self::Bound(_)
            | Self::Brand(_)
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
            Self::Branded(ty, brands) => {
                ty.names(visit);
                for brand in brands {
                    brand.names(visit);
                }
            }
            Self::List(member) | Self::Cell(member) | Self::Iterator(member) => member.names(visit),
            Self::Method { needs, function } => {
                needs.names(visit);
                function.names(visit);
            }
            Self::Generic { bounds, function } => {
                for bound in bounds {
                    bound.names(visit);
                }
                function.names(visit);
            }
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

impl<N: Clone> Type<N> {
    /// The type with every type in it passed through `leaf`, which
    /// gives what to put in its place or nothing to keep it. `leaf` is
    /// told how many parameters of generic function types are bound
    /// around the type it is asked about: `depth`, and more inside.
    #[must_use]
    pub fn rewrite(&self, depth: usize, leaf: &dyn Fn(&Self, usize) -> Option<Self>) -> Self {
        if let Some(replaced) = leaf(self, depth) {
            return replaced;
        }
        let all = |types: &[Self], depth: usize| -> Vec<Self> {
            types.iter().map(|ty| ty.rewrite(depth, leaf)).collect()
        };
        let one = |ty: &Self, depth: usize| Box::new(ty.rewrite(depth, leaf));
        match self {
            Self::Record(fields) => {
                let field = |(key, ty): &(Key<N>, Self)| (key.clone(), ty.rewrite(depth, leaf));
                Self::Record(fields.iter().map(field).collect())
            }
            Self::Tuple(members) => Self::Tuple(all(members, depth)),
            Self::Union(members) => Self::Union(all(members, depth)),
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
            Self::Named(name, args) => Self::Named(name.clone(), all(args, depth)),
            // A bound may mention the parameters it is beside.
            Self::Generic { bounds, function } => Self::Generic {
                bounds: all(bounds, depth + bounds.len()),
                function: one(function, depth + bounds.len()),
            },
            _ => self.clone(),
        }
    }

    /// The parameter of a generic function type that this is, if it is
    /// one of the `count` bound just outside `depth` more.
    #[must_use]
    pub fn bound_at(&self, depth: usize, count: usize) -> Option<usize> {
        match self {
            Self::Bound(index) if (depth..depth + count).contains(index) => Some(index - depth),
            _ => None,
        }
    }

    /// Whether the type mentions no parameter of a generic function
    /// type that is not inside it.
    #[must_use]
    pub fn closed(&self) -> bool {
        let free = std::cell::Cell::new(false);
        let _ = self.rewrite(0, &|ty, depth| {
            if matches!(ty, Self::Bound(index) if *index >= depth) {
                free.set(true);
            }
            None
        });
        !free.get()
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
            Self::Send { method, .. } => method.names(visit),
            Self::Match { arms, .. } => arms.iter().for_each(|arm| arm.pattern.names(visit)),
            Self::Lambda {
                type_params,
                params,
                row,
                returns,
                ..
            } => {
                for bound in type_params.iter().filter_map(|param| param.bound.as_ref()) {
                    bound.names(visit);
                }
                for ty in params.iter().filter_map(|(_, ty)| ty.as_ref()) {
                    ty.names(visit);
                }
                if let Some(returns) = returns {
                    returns.names(visit);
                }
                row.iter().for_each(&mut *visit);
            }
            Self::Cell { ty: Some(ty), .. } | Self::Cast { ty, .. } => ty.names(visit),
            Self::Annotated { key, value, .. } => {
                visit(key);
                value.names(visit);
            }
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
            // What it holds, and what releases that.
            Self::Using { .. } => 2,
            _ => 0,
        };
        let mut inside = 0;
        self.children(&mut |child| inside += child.binders());
        own + inside
    }
}

/// The expressions directly inside one, in order, for a walk that only
/// looks and for one that changes them: written once, so the two agree.
macro_rules! children {
    ($self:expr, $visit:ident, $iter:ident $(, $mutable:tt)?) => {
        match $self {
            Self::Null
            | Self::Literal(_)
            | Self::Name(_)
            | Self::Instantiate { .. }
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
            Self::Tuple(members) | Self::List(members) => members.$iter().for_each($visit),
            Self::Field { target, .. } => $visit(target),
            Self::Method(function) => $visit(function),
            Self::Let { value, body, .. } => {
                $visit(value);
                $visit(body);
            }
            Self::Operation { operands, .. } => operands.$iter().for_each($visit),
            Self::Merge { left, right } => {
                $visit(left);
                $visit(right);
            }
            Self::Match { subject, arms } => {
                $visit(subject);
                for arm in arms {
                    $visit(&$($mutable)? arm.body);
                }
            }
            Self::Lambda { body, .. } | Self::At(_, body) => $visit(body),
            Self::Perform { args, .. } => args.$iter().for_each($visit),
            Self::Provide { bundle, body, .. }
            | Self::Using {
                value: bundle,
                body,
                ..
            } => {
                $visit(bundle);
                $visit(body);
            }
            Self::Observe { observer, body } => {
                $visit(observer);
                $visit(body);
            }
            Self::All(exprs) | Self::First(exprs) | Self::Cell { args: exprs, .. } => {
                exprs.$iter().for_each($visit);
            }
            Self::Comptime(call) | Self::Group(call) | Self::Spawn(call) | Self::Boundary(call) => {
                $visit(call);
            }
            Self::Cast {
                value, otherwise, ..
            } => {
                $visit(value);
                $visit(otherwise);
            }
            // What is said is data, and no part of what is evaluated.
            Self::Annotated { expr, .. } => $visit(expr),
        }
    };
}

impl<N> Expr<N> {
    /// Calls `visit` with the name of every method called by name
    /// inside the expression.
    pub fn sends<'a>(&'a self, visit: &mut impl FnMut(&'a str)) {
        if let Self::Send {
            method: Key::Field(name),
            ..
        } = self
        {
            visit(name);
        }
        self.children(&mut |child| child.sends(visit));
    }

    /// Whether a using is anywhere inside the expression.
    pub fn releases(&self) -> bool {
        let mut found = matches!(self, Self::Using { .. });
        self.children(&mut |child| found = found || child.releases());
        found
    }

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
        children!(self, visit, iter);
    }

    /// [`Self::children`], for a walk that changes them.
    pub fn children_mut(&mut self, visit: &mut impl FnMut(&mut Self)) {
        children!(self, visit, iter_mut, mut);
    }

    /// The node numbered `index` of this tree, counted as
    /// [`Self::index_of`] counts them, if there are that many.
    #[must_use]
    pub fn nth(&self, index: usize) -> Option<&Self> {
        fn seek<'a, N>(at: &'a Expr<N>, left: &mut usize) -> Option<&'a Expr<N>> {
            if *left == 0 {
                return Some(at);
            }
            *left -= 1;
            let mut found = None;
            at.children(&mut |child| found = found.or_else(|| seek(child, left)));
            found
        }
        let mut left = index;
        seek(self, &mut left)
    }

    /// Where `node` is among the nodes of this tree, counted from this
    /// one in the order a walk meets them, if it is one of them.
    pub fn index_of(&self, node: *const Self) -> Option<usize> {
        fn seek<N>(at: &Expr<N>, node: *const Expr<N>, passed: &mut usize) -> bool {
            if std::ptr::eq(at, node) {
                return true;
            }
            *passed += 1;
            let mut found = false;
            at.children(&mut |child| found = found || seek(child, node, passed));
            found
        }
        let mut passed = 0;
        seek(self, node, &mut passed).then_some(passed)
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
        self.annotations.iter().for_each(|said| visit(&said.key));
        match &self.kind {
            DefinitionKind::Type(definition) => definition.ty.names(visit),
            DefinitionKind::Derived(call)
            | DefinitionKind::Generated(call)
            | DefinitionKind::Module(call) => call.names(visit),
            DefinitionKind::Function(function) => {
                function.header_names(visit);
                function.body.names(visit);
            }
            DefinitionKind::Key(key) => key.ty.names(visit),
            DefinitionKind::Tag | DefinitionKind::Brand | DefinitionKind::Intrinsic => {}
            DefinitionKind::Ability(ability) => {
                for bound in ability.type_params.iter().filter_map(|p| p.bound.as_ref()) {
                    bound.names(visit);
                }
                ability
                    .params
                    .iter()
                    .for_each(|param| param.ty.names(visit));
                ability.ret.names(visit);
            }
            DefinitionKind::Extern(native) => match &native.kind {
                ExternKind::Function { params, ret, row } => {
                    for param in params {
                        param.ty.names(visit);
                    }
                    ret.names(visit);
                    row.iter().for_each(visit);
                }
                ExternKind::Handler { ability, row } => {
                    visit(ability);
                    row.iter().for_each(visit);
                }
            },
            DefinitionKind::Target(target) => {
                target.extends.iter().for_each(&mut *visit);
                target.tool.iter().for_each(&mut *visit);
                for (ability, handler) in &target.bindings {
                    visit(ability);
                    visit(handler);
                }
            }
        }
    }
}
