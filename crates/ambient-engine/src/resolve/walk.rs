//! AST traversal for the resolve pass: the mechanical recursion over
//! items, expressions, statements, and patterns, plus the lexical-scope
//! stack and block-scoped `use` overlay management. This code decides
//! *where* references live, never *what* they mean (that is `refs`).

use std::sync::Arc;

use crate::ast::{
    Expr, ExprKind, ItemKind, Module, Pattern, PatternKind, QualifiedName, Stmt, StmtKind,
};
use crate::module_path::ModulePath;
use crate::module_registry::{ImportError, ModuleScope};

use super::Resolver;

impl Resolver<'_> {
    pub(super) fn resolve(&mut self, module: &mut Module) {
        // Split the borrow: the resolver holds no reference into `module`.
        for item in &mut module.items {
            self.resolve_item(&mut item.kind);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_item(&mut self, item: &mut ItemKind) {
        match item {
            ItemKind::Function(f) => {
                self.push_type_params(&f.type_params);
                for ability in &mut f.abilities {
                    self.resolve_ability_ref(ability);
                }
                for param in &mut f.params {
                    if let Some(ty) = &mut param.ty {
                        self.resolve_type(ty);
                    }
                }
                if let Some(ty) = &mut f.ret_ty {
                    self.resolve_type(ty);
                }
                self.push_scope(f.params.iter().map(|p| Arc::clone(&p.name)).collect());
                self.resolve_expr(&mut f.body);
                self.pop_scope();
                self.pop_type_params();
            }
            ItemKind::Const(c) => {
                if let Some(ty) = &mut c.ty {
                    self.resolve_type(ty);
                }
                self.resolve_expr(&mut c.value);
            }
            ItemKind::Ability(a) => {
                for dep in &mut a.dependencies {
                    self.resolve_ability_ref(dep);
                }
                for method in &mut a.methods {
                    self.push_type_params(&method.type_params);
                    for param in &mut method.params {
                        if let Some(ty) = param.ty.as_mut() {
                            self.resolve_type(ty);
                        }
                    }
                    self.resolve_type(&mut method.ret_ty);
                    // The default implementation is an ordinary body: its
                    // references canonicalize exactly like a function's.
                    if let Some(body) = &mut method.body {
                        self.push_scope(
                            method.params.iter().map(|p| Arc::clone(&p.name)).collect(),
                        );
                        self.resolve_expr(body);
                        self.pop_scope();
                    }
                    self.pop_type_params();
                }
            }
            ItemKind::Impl(imp) => {
                // The block's own params (`impl<T> Foo<T>`) scope the
                // `for` type and every method; each method's own params
                // nest inside for that method's signature and body.
                self.push_type_params(&imp.type_params);
                self.resolve_type(&mut imp.for_type);
                for method in &mut imp.methods {
                    self.push_type_params(&method.type_params);
                    for ability in &mut method.abilities {
                        self.resolve_ability_ref(ability);
                    }
                    for param in &mut method.params {
                        if let Some(ty) = &mut param.ty {
                            self.resolve_type(ty);
                        }
                    }
                    if let Some(ty) = &mut method.ret_ty {
                        self.resolve_type(ty);
                    }
                    let mut names: Vec<Arc<str>> =
                        method.params.iter().map(|p| Arc::clone(&p.name)).collect();
                    if method.has_self {
                        names.push(Arc::from("self"));
                    }
                    self.push_scope(names);
                    self.resolve_expr(&mut method.body);
                    self.pop_scope();
                    self.pop_type_params();
                }
                self.pop_type_params();
            }
            ItemKind::Struct(s) => {
                self.push_type_params(&s.type_params);
                self.resolve_type(&mut s.ty);
                self.pop_type_params();
            }
            ItemKind::TypeAlias(t) => {
                self.push_type_params(&t.type_params);
                self.resolve_type(&mut t.ty);
                self.pop_type_params();
            }
            ItemKind::Enum(e) => {
                self.push_type_params(&e.type_params);
                for variant in &mut e.variants {
                    if let Some(payload) = &mut variant.payload {
                        self.resolve_type(payload);
                    }
                }
                self.pop_type_params();
            }
            ItemKind::Trait(t) => {
                // Trait-level params (`trait Container<T>`) scope every
                // method signature; each method's own params nest inside.
                self.push_type_params(&t.type_params);
                for method in &mut t.methods {
                    self.push_type_params(&method.type_params);
                    for (_, ty) in &mut method.params {
                        self.resolve_type(ty);
                    }
                    self.resolve_type(&mut method.ret_ty);
                    self.pop_type_params();
                }
                self.pop_type_params();
            }
            ItemKind::ExternFn(e) => {
                self.push_type_params(&e.type_params);
                for param in &mut e.params {
                    if let Some(ty) = &mut param.ty {
                        self.resolve_type(ty);
                    }
                }
                self.resolve_type(&mut e.ret_ty);
                self.pop_type_params();
            }
            ItemKind::Use(_) => {}
        }
    }

    fn push_scope(&mut self, names: Vec<Arc<str>>) {
        self.locals.push(names);
    }

    fn pop_scope(&mut self) {
        self.locals.pop();
    }

    fn push_type_params(&mut self, params: &[crate::ast::TypeParam]) {
        self.type_params
            .push(params.iter().map(|p| Arc::clone(&p.name)).collect());
    }

    fn pop_type_params(&mut self) {
        self.type_params.pop();
    }

    /// Resolve the variant constructors named inside a match-arm pattern.
    ///
    /// A variant pattern (`Some(x)`, `shapes::Circle`) names a variant at a
    /// pattern position exactly as a constructor expression names it at a
    /// value position, so it goes through the *same* `resolve_value_ref`
    /// machinery and lands on the same two-segment ident
    /// `Fqn(module, [Enum, Variant])`. The checker then picks the enum and
    /// variant by that `Fqn` rather than by a bare-name reverse lookup, so a
    /// variant name shared by two enums never mis-dispatches. Binding names
    /// inside the pattern are locals (handled by `collect_pattern_bindings`),
    /// not references.
    fn resolve_pattern(&mut self, pattern: &mut Pattern) {
        match &mut pattern.kind {
            PatternKind::Wildcard | PatternKind::Binding(..) | PatternKind::Literal(_) => {}
            PatternKind::Tuple(elems) => {
                for elem in elems {
                    self.resolve_pattern(elem);
                }
            }
            PatternKind::Record(fields) => {
                for (_, field) in fields {
                    self.resolve_pattern(field);
                }
            }
            PatternKind::Variant(name, payload) => {
                self.resolve_value_ref(name);
                if let Some(payload) = payload {
                    self.resolve_pattern(payload);
                }
            }
        }
    }

    fn declare_local(&mut self, name: Arc<str>) {
        if let Some(top) = self.locals.last_mut() {
            top.push(name);
        } else {
            self.locals.push(vec![name]);
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // AST walking
    // ─────────────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    fn resolve_expr(&mut self, expr: &mut Expr) {
        // Module-alias method-call disambiguation: `utils.helper(x)` parses
        // as a method call on the value `utils`, but when `utils` is a
        // module alias (and not a local), it is a qualified call — rewrite
        // the node so the checker and compiler see it that way.
        let rewrite = if let ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } = &mut expr.kind
        {
            if let ExprKind::Name(name) = &receiver.kind {
                let target = (name.path.is_empty()
                    && !self.is_local(&name.name)
                    && !self.module_values.contains(&name.name)
                    && !self.module_variants.contains_key(&name.name))
                .then(|| self.scope_module(&name.name).cloned())
                .flatten()
                .filter(|module| self.item_exists(module, method));
                target.map(|module| {
                    let mut callee_name =
                        QualifiedName::qualified(vec![Arc::clone(&name.name)], Arc::clone(method));
                    callee_name.resolved = Some(self.canonical(&module, vec![Arc::clone(method)]));
                    let callee = Expr {
                        kind: ExprKind::Name(callee_name),
                        span: receiver.span,
                        ty: None,
                        dicts: None,
                    };
                    ExprKind::Call(Box::new(callee), std::mem::take(args))
                })
            } else {
                None
            }
        } else {
            None
        };
        if let Some(kind) = rewrite {
            expr.kind = kind;
        }

        match &mut expr.kind {
            ExprKind::Unit
            | ExprKind::Bool(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Local(_) => {}

            ExprKind::Name(name) => self.resolve_value_ref(name),

            ExprKind::Tuple(elems) | ExprKind::List(elems) => {
                for elem in elems {
                    self.resolve_expr(elem);
                }
            }
            ExprKind::TupleIndex(inner, _) | ExprKind::RecordField(inner, _) => {
                self.resolve_expr(inner);
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.resolve_expr(value);
                }
            }
            ExprKind::TypedRecord { type_name, fields } => {
                self.resolve_type_ref(type_name);
                for (_, value) in fields {
                    self.resolve_expr(value);
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                self.resolve_expr(receiver);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Unary(_, inner) | ExprKind::Resume(inner) => self.resolve_expr(inner),
            ExprKind::If(cond, then, els) => {
                self.resolve_expr(cond);
                self.resolve_expr(then);
                if let Some(els) = els {
                    self.resolve_expr(els);
                }
            }
            ExprKind::Match(scrutinee, arms) => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    self.resolve_pattern(&mut arm.pattern);
                    let mut bindings = Vec::new();
                    collect_pattern_bindings(&arm.pattern, &mut bindings);
                    self.push_scope(bindings);
                    if let Some(guard) = &mut arm.guard {
                        self.resolve_expr(guard);
                    }
                    self.resolve_expr(&mut arm.body);
                    self.pop_scope();
                }
            }
            ExprKind::Block(stmts, result) => {
                self.push_scope(Vec::new());
                self.overlays.push(ModuleScope::default());
                for stmt in stmts {
                    self.resolve_stmt(stmt);
                }
                if let Some(result) = result {
                    self.resolve_expr(result);
                }
                self.overlays.pop();
                self.pop_scope();
            }
            ExprKind::Lambda(lambda) => {
                for param in &mut lambda.params {
                    if let Some(ty) = &mut param.ty {
                        self.resolve_type(ty);
                    }
                }
                self.push_scope(lambda.params.iter().map(|p| Arc::clone(&p.name)).collect());
                self.resolve_expr(&mut lambda.body);
                self.pop_scope();
            }
            ExprKind::Call(callee, args) => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Perform(call) => {
                self.resolve_ability_ref(&mut call.ability);
                for arg in &mut call.args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Handle(handle) => {
                // Each handler is an ordinary expression (literal or value);
                // resolving it recurses into HandlerLiteral below.
                for handler in &mut handle.handlers {
                    self.resolve_expr(handler);
                }
                self.resolve_expr(&mut handle.body);
                if let Some(els) = &mut handle.else_clause {
                    self.resolve_expr(els);
                }
            }
            ExprKind::HandlerLiteral(literal) => {
                for method in &mut literal.methods {
                    self.resolve_ability_ref(&mut method.ability);
                    self.push_scope(method.params.iter().map(|p| Arc::clone(&p.name)).collect());
                    self.resolve_expr(&mut method.body);
                    self.pop_scope();
                }
            }
            ExprKind::Sandbox(sandbox) => {
                for ability in &mut sandbox.allowed_abilities {
                    self.resolve_ability_ref(ability);
                }
                self.resolve_expr(&mut sandbox.body);
            }
        }
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Let(binding) => {
                if let Some(ty) = &mut binding.ty {
                    self.resolve_type(ty);
                }
                self.resolve_expr(&mut binding.init);
                self.declare_local(Arc::clone(&binding.name));
            }
            StmtKind::Expr(expr) => self.resolve_expr(expr),
            StmtKind::Use(use_def) => self.bind_block_use(use_def, stmt.span),
            StmtKind::Const(const_def) => {
                if let Some(ty) = &mut const_def.ty {
                    self.resolve_type(ty);
                }
                self.resolve_expr(&mut const_def.value);
                // Bind the name lexically, exactly like `let`: it shadows
                // outer bindings from here to the end of the block, and a
                // reference *before* this point stays unresolved (an
                // undefined-name error), so no forward-reference pass is
                // needed.
                self.declare_local(Arc::clone(&const_def.name));
            }
        }
    }

    /// Bind a block-scoped `use` into the innermost overlay. Semantics
    /// match a module-level `use`, except the binding ends with the
    /// enclosing block and `Local` heads may resolve through any visible
    /// scope (outer blocks, then the module scope).
    fn bind_block_use(&mut self, use_def: &crate::ast::UseDef, span: crate::ast::Span) {
        use crate::ast::UsePrefix;

        let path_names: Vec<Arc<str>> = use_def.path.iter().map(|(name, _)| name.clone()).collect();
        let target = if use_def.prefix == UsePrefix::Local {
            let Some(head) = path_names.first() else {
                return;
            };
            let Some(base) = self.scope_module(head).cloned() else {
                self.import_errors.push(ImportError {
                    error: crate::module_registry::RegistryError::UnresolvedHead {
                        head: head.to_string(),
                    },
                    span,
                });
                return;
            };
            let mut segments = base.segments().to_vec();
            segments.extend(path_names[1..].iter().cloned());
            match ModulePath::from_segments(segments) {
                Some(target) => target,
                None => return,
            }
        } else {
            match self
                .registry
                .resolve_use_path(self.current, &use_def.prefix, &path_names)
            {
                Ok(target) => target,
                Err(error) => {
                    self.import_errors.push(ImportError { error, span });
                    return;
                }
            }
        };

        let mut bound = ModuleScope::default();
        self.registry
            .bind_use_target(&mut bound, use_def, &target, span);
        self.import_errors.append(&mut bound.errors);
        if let Some(overlay) = self.overlays.last_mut() {
            for (name, module) in bound.modules {
                overlay.modules.insert(name, module);
            }
            for (name, imports) in bound.items {
                overlay.items.insert(name, imports);
            }
        }
    }
}

/// Collect the names a pattern binds.
fn collect_pattern_bindings(pattern: &Pattern, out: &mut Vec<Arc<str>>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Binding(_, name) => out.push(Arc::clone(name)),
        PatternKind::Tuple(elems) => {
            for elem in elems {
                collect_pattern_bindings(elem, out);
            }
        }
        PatternKind::Record(fields) => {
            for (_, field) in fields {
                collect_pattern_bindings(field, out);
            }
        }
        PatternKind::Variant(_, payload) => {
            if let Some(payload) = payload {
                collect_pattern_bindings(payload, out);
            }
        }
    }
}
