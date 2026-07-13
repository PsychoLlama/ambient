//! Occurrence index: every definition and reference site of every symbol,
//! with exact source spans.
//!
//! This is what powers find-references and rename. It is computed by walking
//! the AST once and, for every name, deciding the site's target:
//!
//! - **Cross-module / module-item names** resolve through
//!   [`queries::resolve_qualified_name`], i.e. the engine's `ModuleRegistry` —
//!   the single source of import and export truth. This walk never
//!   re-implements that resolution.
//! - **Local bindings** (params, `let`, patterns) are tracked by lexical
//!   scope using the binding ids the parser already assigned. This is
//!   necessary because the checker resolves a bare local reference by name
//!   without recording *which* binding it hit in the AST, so the id is not
//!   otherwise recoverable at the use site. The scope walk only reads the
//!   AST's own binding structure; it makes no cross-module decisions.
//!
//! Ownership matters: these lists live in `ambient-analysis`, not the LSP. The
//! LSP is a renderer that turns [`Occurrence`]s into reference locations and
//! rename edits — it never resolves a name itself.
//!
//! The walk runs on the parsed (recovering) AST the registry already holds,
//! so it costs the compile pipeline nothing (`build_package` never calls it)
//! and refreshes with the per-edit registry rebuild.
//!
//! ## Scope (v1)
//!
//! Indexed: module-level items (functions, consts, type aliases, enums,
//! abilities, traits) at their name span; references to them in expression
//! position (`Name`/`Call`/`TypedRecord`) and in `use` imports; and local
//! bindings with their same-file references.
//!
//! Not yet indexed (so references to these are incomplete — the LSP gates
//! rename to functions/consts/locals accordingly): enum *variant*
//! constructors and patterns, method-call / operator dispatch, and type
//! references in signatures and annotations. These are natural follow-ups.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::ast::{
    BindingId, Expr, ExprKind, Item, ItemKind, Module, Param, Pattern, PatternKind, QualifiedName,
    Span, Stmt, StmtKind, UseDef,
};
use ambient_engine::fqn::Fqn;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;

use crate::queries::{Definition, resolve_qualified_name};

/// The definition an occurrence points at — the identity used to group
/// occurrences of "the same thing" across the package.
///
/// Equality is deliberately structural on identity only (`Item` on its
/// fully-qualified [`Fqn`], `Local` on module + binding id); the carried
/// `module`/`name` are metadata for rendering and rename/collision checks and
/// are not compared. This mirrors how the AST's `QualifiedName` ignores spans
/// in its own equality.
///
/// Keying [`Item`](Self::Item) on the [`Fqn`] rather than a definition span is
/// what lets the incremental session rebuild only the edited module's
/// occurrences: a body edit that shifts a definition's span leaves its `Fqn`
/// (module identity + ident path) untouched, so every *other* module's
/// references to it stay valid without a re-walk. Every resolved reference and
/// the definition itself canonicalize to the same `Fqn` (via
/// [`ModuleRegistry::fqn`]), so a definition and all its references collapse to
/// one identity — the same canonicalization the engine's resolve pass performs.
#[derive(Debug, Clone)]
pub enum SymbolTarget {
    /// A module-level item, identified by its fully-qualified [`Fqn`]. The
    /// `Fqn` is what [`queries::resolve_qualified_name`] resolves a reference to
    /// (canonicalized to the item's origin module), so a definition and all its
    /// references — across every module — collapse to one identity.
    Item {
        /// The item's fully-qualified identity (the sole equality key).
        fqn: Fqn,
        /// The module that defines the item (metadata for the renderer, which
        /// must map back to the registry/package — both `ModulePath`-keyed —
        /// and for same-module gating; not part of identity).
        module: ModulePath,
        /// The item's name (metadata; not part of identity).
        name: Arc<str>,
    },
    /// A local binding (parameter, `let`, or pattern binding), unique within
    /// one module. Renamed and referenced same-file only.
    Local {
        /// The module the binding lives in.
        module: ModulePath,
        /// The binding id assigned during lowering (module-unique).
        binding_id: BindingId,
        /// The binding's name (metadata; not part of identity).
        name: Arc<str>,
    },
}

impl SymbolTarget {
    /// The symbol's name (the item or binding name).
    #[must_use]
    pub fn name(&self) -> &Arc<str> {
        match self {
            Self::Item { name, .. } | Self::Local { name, .. } => name,
        }
    }

    /// The module the symbol is defined in.
    #[must_use]
    pub fn module(&self) -> &ModulePath {
        match self {
            Self::Item { module, .. } | Self::Local { module, .. } => module,
        }
    }

    /// Whether this is a local binding (as opposed to a module-level item).
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

impl PartialEq for SymbolTarget {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Item { fqn: f1, .. }, Self::Item { fqn: f2, .. }) => f1 == f2,
            (
                Self::Local {
                    module: m1,
                    binding_id: b1,
                    ..
                },
                Self::Local {
                    module: m2,
                    binding_id: b2,
                    ..
                },
            ) => m1 == m2 && b1 == b2,
            _ => false,
        }
    }
}

impl Eq for SymbolTarget {}

/// One (definition | reference) site with an exact source range.
#[derive(Debug, Clone)]
pub struct Occurrence {
    /// The exact byte range of the identifier at this site — what an editor
    /// highlights and what rename rewrites.
    pub span: Span,
    /// The symbol this site refers to.
    pub target: SymbolTarget,
    /// True at the symbol's own definition site.
    pub is_definition: bool,
}

/// Collect every occurrence in `module`, resolving cross-module names through
/// `registry`. `module_path` is the path `module` is registered under.
///
/// Pure AST walk; safe on the recovering (partial) AST the editor analyzes.
#[must_use]
pub fn collect_occurrences(
    module: &Module,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Vec<Occurrence> {
    let mut collector = Collector {
        module,
        module_path,
        registry,
        scopes: Vec::new(),
        out: Vec::new(),
    };
    for item in &module.items {
        collector.item(item);
    }
    collector.out
}

struct Collector<'a> {
    module: &'a Module,
    module_path: &'a ModulePath,
    registry: &'a ModuleRegistry,
    /// Lexical scope stack: innermost last. Each scope maps a binding name to
    /// its id, so a bare-name reference finds the local it shadows to.
    scopes: Vec<HashMap<Arc<str>, BindingId>>,
    out: Vec<Occurrence>,
}

impl Collector<'_> {
    /// The name span for a definition whose node span may include trailing
    /// syntax (`x: number`): identifiers begin at `start` and span exactly
    /// their source bytes, so the end is `start + name.len()`.
    fn name_span_at(start: u32, name: &str) -> Span {
        let len = u32::try_from(name.len()).unwrap_or(0);
        Span::new(start, start + len)
    }

    // ── scope management ────────────────────────────────────────────────────

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Look up a bare name in the lexical scope stack, innermost first.
    fn lookup_local(&self, name: &str) -> Option<BindingId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    /// Bind a local in the current scope and record its definition occurrence.
    fn bind_local(&mut self, id: BindingId, name: &Arc<str>, span: Span) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(Arc::clone(name), id);
        }
        self.out.push(Occurrence {
            span,
            target: SymbolTarget::Local {
                module: self.module_path.clone(),
                binding_id: id,
                name: Arc::clone(name),
            },
            is_definition: true,
        });
    }

    fn bind_param(&mut self, param: &Param) {
        let span = Self::name_span_at(param.span.start, &param.name);
        self.bind_local(param.id, &param.name, span);
    }

    // ── occurrence emission ─────────────────────────────────────────────────

    /// Record a module-item definition at its name span.
    fn item_def(&mut self, name: &Arc<str>, name_span: Span) {
        let target = self.item_target_in(self.module_path, name);
        self.out.push(Occurrence {
            span: name_span,
            target,
            is_definition: true,
        });
    }

    /// Turn a resolved [`Definition`] into an `Item` target (a same-module
    /// `None` maps to the current module).
    fn item_target(&self, def: &Definition, name: &Arc<str>) -> SymbolTarget {
        let module = def.module.as_ref().unwrap_or(self.module_path);
        self.item_target_in(module, name)
    }

    /// Build an `Item` target for `name` defined in `module`, canonicalizing
    /// its identity to the [`Fqn`] the registry mints for it — the same identity
    /// a resolved reference lands on, so definition and references collapse.
    fn item_target_in(&self, module: &ModulePath, name: &Arc<str>) -> SymbolTarget {
        SymbolTarget::Item {
            fqn: self.registry.fqn(module, std::slice::from_ref(name)),
            module: module.clone(),
            name: Arc::clone(name),
        }
    }

    /// Resolve a name reference through the registry and, if it lands on a
    /// package item, record a reference occurrence at `span`.
    fn name_ref(&mut self, qname: &QualifiedName, span: Span) {
        if let Some(def) =
            resolve_qualified_name(self.module, self.module_path, self.registry, qname)
        {
            let target = self.item_target(&def, &qname.name);
            self.out.push(Occurrence {
                span,
                target,
                is_definition: false,
            });
        }
    }

    fn local_ref(&mut self, id: BindingId, name: &Arc<str>, span: Span) {
        self.out.push(Occurrence {
            span,
            target: SymbolTarget::Local {
                module: self.module_path.clone(),
                binding_id: id,
                name: Arc::clone(name),
            },
            is_definition: false,
        });
    }

    // ── walk ────────────────────────────────────────────────────────────────

    fn item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Function(f) => {
                self.item_def(&f.name, f.name_span);
                self.push_scope();
                for param in &f.params {
                    self.bind_param(param);
                }
                self.expr(&f.body);
                self.pop_scope();
            }
            ItemKind::Const(c) => {
                self.item_def(&c.name, c.name_span);
                self.push_scope();
                self.expr(&c.value);
                self.pop_scope();
            }
            ItemKind::Struct(s) => self.item_def(&s.name, s.name_span),
            ItemKind::TypeAlias(t) => self.item_def(&t.name, t.name_span),
            ItemKind::Enum(e) => self.item_def(&e.name, e.name_span),
            ItemKind::Ability(a) => self.item_def(&a.name, a.name_span),
            ItemKind::Trait(t) => self.item_def(&t.name, t.name_span),
            ItemKind::ExternFn(e) => self.item_def(&e.name, e.name_span),
            ItemKind::Use(use_def) => self.use_items(use_def),
            ItemKind::Impl(impl_def) => {
                for method in &impl_def.methods {
                    // The method name is not indexed (method rename is a
                    // follow-up), but its body references package items and
                    // locals that must be found.
                    self.push_scope();
                    if method.has_self
                        && let Some(scope) = self.scopes.last_mut()
                    {
                        scope.insert(Arc::from("self"), method.self_id);
                    }
                    for param in &method.params {
                        self.bind_param(param);
                    }
                    self.expr(&method.body);
                    self.pop_scope();
                }
            }
        }
    }

    /// Record reference occurrences for the symbols a `use` item imports.
    ///
    /// Use trees are flattened during lowering, so every `UseDef` names one
    /// entity by its final path segment; it counts as a symbol occurrence
    /// only when it resolves as one (a whole-module import is not a symbol
    /// occurrence).
    fn use_items(&mut self, use_def: &UseDef) {
        if let Some((name, span)) = use_def.path.last() {
            self.import_ref(name, *span);
        }
    }

    /// Resolve a bare imported name to its origin and, if it is a symbol,
    /// record a reference occurrence at the import site.
    fn import_ref(&mut self, name: &Arc<str>, span: Span) {
        let qname = QualifiedName::simple(Arc::clone(name));
        self.name_ref(&qname, span);
    }

    #[allow(clippy::too_many_lines)]
    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Unit
            | ExprKind::Bool(_)
            | ExprKind::Number(_)
            | ExprKind::String(_)
            | ExprKind::Perform(_) => {}

            // Produced only by builders today (the parser lowers bare locals
            // to `Name`); handled for completeness.
            ExprKind::Local(id) => {
                let name = Arc::from("");
                self.local_ref(*id, &name, expr.span);
            }

            ExprKind::Name(qname) => {
                let span = qname.name_span.unwrap_or(expr.span);
                if qname.path.is_empty()
                    && let Some(id) = self.lookup_local(&qname.name)
                {
                    self.local_ref(id, &qname.name, span);
                } else {
                    self.name_ref(qname, span);
                }
            }

            ExprKind::Tuple(elems) | ExprKind::List(elems) => {
                for e in elems {
                    self.expr(e);
                }
            }
            ExprKind::Record(fields) => {
                for (_, e) in fields {
                    self.expr(e);
                }
            }
            ExprKind::TypedRecord { type_name, fields } => {
                if let Some(span) = type_name.name_span {
                    self.name_ref(type_name, span);
                }
                for (_, e) in fields {
                    self.expr(e);
                }
            }
            ExprKind::RecordField(obj, _) => self.expr(obj),
            ExprKind::MethodCall { receiver, args, .. } => {
                self.expr(receiver);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::TupleIndex(e, _) | ExprKind::Unary(_, e) | ExprKind::Resume(e) => {
                self.expr(e);
            }
            ExprKind::If(c, t, e) => {
                self.expr(c);
                self.expr(t);
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            ExprKind::Match(scrut, arms) => {
                self.expr(scrut);
                for arm in arms {
                    self.push_scope();
                    self.pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&arm.body);
                    self.pop_scope();
                }
            }
            ExprKind::Block(stmts, result) => {
                self.push_scope();
                for stmt in stmts {
                    self.stmt(stmt);
                }
                if let Some(result) = result {
                    self.expr(result);
                }
                self.pop_scope();
            }
            ExprKind::Lambda(lambda) => {
                self.push_scope();
                for param in &lambda.params {
                    self.bind_param(param);
                }
                self.expr(&lambda.body);
                self.pop_scope();
            }
            ExprKind::Call(callee, args) => {
                self.expr(callee);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Handle(handle) => {
                // Each handler is an ordinary expression (a literal or a
                // value); walking it recurses into HandlerLiteral below.
                for handler in &handle.handlers {
                    self.expr(handler);
                }
                self.expr(&handle.body);
                if let Some(else_clause) = &handle.else_clause {
                    self.expr(else_clause);
                }
            }
            ExprKind::HandlerLiteral(literal) => {
                for method in &literal.methods {
                    self.push_scope();
                    for param in &method.params {
                        self.bind_param(param);
                    }
                    self.expr(&method.body);
                    self.pop_scope();
                }
            }
            ExprKind::Sandbox(sandbox) => self.expr(&sandbox.body),
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(binding) => {
                // The initializer is checked in the enclosing scope, so walk
                // it before binding — a `let x = x` shadow's right-hand `x`
                // refers to the outer binding.
                self.expr(&binding.init);
                self.bind_local(binding.id, &binding.name, binding.name_span);
            }
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Use(use_def) => self.use_items(use_def),
            StmtKind::Const(const_def) => {
                // A block `const` binds its name lexically, just like `let`:
                // walk the value first, then bind the name so references from
                // here on resolve to it.
                self.expr(&const_def.value);
                self.bind_local(const_def.id, &const_def.name, const_def.name_span);
            }
        }
    }

    fn pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Binding(id, name) => self.bind_local(*id, name, pattern.span),
            PatternKind::Tuple(elems) => {
                for p in elems {
                    self.pattern(p);
                }
            }
            PatternKind::Record(fields) => {
                for (_, p) in fields {
                    self.pattern(p);
                }
            }
            PatternKind::Variant(_, payload) => {
                // The variant constructor reference is a follow-up; still bind
                // its payload's patterns.
                if let Some(payload) = payload {
                    self.pattern(payload);
                }
            }
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect occurrences for a single source as the package root.
    fn occurrences_of(source: &str) -> Vec<Occurrence> {
        let recovered = ambient_parser::parse_recovering(source);
        let module_path = ModulePath::root();
        let mut registry = crate::core_platform_registry();
        registry.register(&module_path, std::sync::Arc::new(recovered.module.clone()));
        collect_occurrences(&recovered.module, &module_path, &registry)
    }

    fn find<'a>(occ: &'a [Occurrence], name: &str, is_def: bool) -> Vec<&'a Occurrence> {
        occ.iter()
            .filter(|o| o.target.name().as_ref() == name && o.is_definition == is_def)
            .collect()
    }

    #[test]
    fn function_definition_and_call_share_a_target() {
        let occ = occurrences_of("fn helper(): Number { 1 }\nfn run(): Number { helper() }");
        let def = find(&occ, "helper", true);
        let refs = find(&occ, "helper", false);
        assert_eq!(def.len(), 1);
        assert_eq!(refs.len(), 1);
        assert_eq!(def[0].target, refs[0].target);
    }

    #[test]
    fn local_param_and_uses_share_a_target() {
        let occ = occurrences_of("fn run(x: Number): Number { x + x }");
        let all: Vec<_> = occ
            .iter()
            .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
            .collect();
        assert_eq!(all.len(), 3, "one param def + two uses");
        assert_eq!(all.iter().filter(|o| o.is_definition).count(), 1);
        for o in &all {
            assert_eq!(o.target, all[0].target);
        }
    }

    #[test]
    fn param_definition_span_excludes_the_type() {
        let occ = occurrences_of("fn run(count: Number): Number { count }");
        let def = find(&occ, "count", true);
        assert_eq!(def.len(), 1);
        // "fn run(" is 7 bytes; `count` spans [7, 12), not `count: number`.
        assert_eq!(def[0].span, Span::new(7, 12));
    }

    #[test]
    fn let_shadow_rhs_refers_to_outer_binding() {
        // `let y = x; ...` — the two `x` uses are the param; `y` is distinct.
        let occ = occurrences_of("fn run(x: Number): Number { let y = x; y }");
        let xs: Vec<_> = occ
            .iter()
            .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
            .collect();
        assert_eq!(xs.len(), 2, "param def + one use in the initializer");
        let ys: Vec<_> = occ
            .iter()
            .filter(|o| o.target.is_local() && o.target.name().as_ref() == "y")
            .collect();
        assert_eq!(ys.len(), 2, "let def + one use in the result");
        assert_ne!(xs[0].target, ys[0].target);
    }

    #[test]
    fn item_identity_is_span_independent() {
        // The whole point of Fqn keying: shifting a definition's span (here by
        // leading blank lines) must not change its target identity, so another
        // module's reference — built at a different revision — still matches.
        let a = occurrences_of("fn helper(): Number { 1 }\nfn run(): Number { helper() }");
        let b = occurrences_of("\n\nfn helper(): Number { 1 }\nfn run(): Number { helper() }");
        let da = find(&a, "helper", true);
        let db = find(&b, "helper", true);
        assert_eq!(da.len(), 1);
        assert_eq!(db.len(), 1);
        // The definition spans differ (b is shifted by two newlines)...
        assert_ne!(da[0].span, db[0].span);
        // ...but the identities are equal, because they key on the Fqn.
        assert_eq!(da[0].target, db[0].target);
        // And a reference in b still collapses onto that identity.
        let rb = find(&b, "helper", false);
        assert_eq!(rb.len(), 1);
        assert_eq!(rb[0].target, da[0].target);
    }

    #[test]
    fn inner_shadow_is_a_distinct_binding() {
        // The lambda's `x` shadows the param `x`: two distinct targets.
        let occ = occurrences_of("fn run(x: Number): Number { let f = (x) => x + 1; x }");
        let targets: std::collections::HashSet<_> = occ
            .iter()
            .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
            .map(|o| match &o.target {
                SymbolTarget::Local { binding_id, .. } => *binding_id,
                SymbolTarget::Item { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            targets.len(),
            2,
            "param x and lambda x are different bindings"
        );
    }
}
