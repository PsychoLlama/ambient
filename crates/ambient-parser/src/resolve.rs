//! Name resolution for the Ambient language.
//!
//! This module implements the name resolution pass that happens after lowering
//! CST to AST. It resolves all qualified names to their definitions and checks
//! for undefined names and duplicate definitions.
//!
//! # Resolution Strategy
//!
//! 1. **Module-level scope**: Functions, constants, types, enums, abilities
//! 2. **Import scope**: Items imported via `use` statements
//! 3. **Local scope**: Parameters and let bindings within functions
//!
//! Resolution happens in multiple passes:
//! 1. Collect all module-level definitions
//! 2. Process imports and build import table
//! 3. Resolve names within function bodies (including local scopes)

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::ast::{
    BindingId, Expr, ExprKind, FunctionDef, ImplMethod, Item, ItemKind, Module, QualifiedName,
    Stmt, StmtKind,
};

use crate::error::{ParseError, ParseErrorKind};

/// A reference to a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefId {
    /// A module-level function.
    Function(Arc<str>),
    /// A module-level constant.
    Const(Arc<str>),
    /// A type alias.
    TypeAlias(Arc<str>),
    /// An enum.
    Enum(Arc<str>),
    /// An enum variant.
    EnumVariant {
        enum_name: Arc<str>,
        variant_name: Arc<str>,
    },
    /// An ability.
    Ability(Arc<str>),
    /// A trait.
    Trait(Arc<str>),
    /// A local binding (parameter or let binding).
    Local(BindingId),
}

/// A symbol table for a single scope.
#[derive(Debug, Clone)]
struct Scope {
    /// Mapping from names to definition IDs.
    bindings: HashMap<Arc<str>, DefId>,
}

impl Scope {
    fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    fn insert(&mut self, name: Arc<str>, def: DefId) -> Result<(), Arc<str>> {
        if self.bindings.contains_key(&name) {
            return Err(name);
        }
        self.bindings.insert(name, def);
        Ok(())
    }

    fn get(&self, name: &str) -> Option<&DefId> {
        self.bindings.get(name)
    }
}

/// The name resolver maintains symbol tables and resolves names.
pub struct Resolver {
    /// Module-level scope (functions, constants, types, etc.)
    module_scope: Scope,
    /// Import scope (items brought in via `use`)
    import_scope: Scope,
    /// Stack of local scopes (for function parameters and let bindings)
    local_scopes: Vec<Scope>,
}

impl Resolver {
    /// Create a new resolver.
    #[must_use]
    pub fn new() -> Self {
        Self {
            module_scope: Scope::new(),
            import_scope: Scope::new(),
            local_scopes: Vec::new(),
        }
    }

    /// Resolve all names in a module.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A name is undefined
    /// - A name is defined multiple times in the same scope
    /// - An import refers to a non-existent item
    pub fn resolve_module(&mut self, module: &Module) -> Result<(), ParseError> {
        // Pass 1: Collect module-level definitions
        self.collect_definitions(module)?;

        // Pass 2: Process imports
        self.process_imports(module)?;

        // Pass 3: Resolve names within each item
        for item in &module.items {
            self.resolve_item(item)?;
        }

        Ok(())
    }

    /// Collect all module-level definitions.
    fn collect_definitions(&mut self, module: &Module) -> Result<(), ParseError> {
        for item in &module.items {
            match &item.kind {
                ItemKind::Function(f) => {
                    self.module_scope
                        .insert(f.name.clone(), DefId::Function(f.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;
                }
                ItemKind::Const(c) => {
                    self.module_scope
                        .insert(c.name.clone(), DefId::Const(c.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;
                }
                ItemKind::TypeAlias(t) => {
                    self.module_scope
                        .insert(t.name.clone(), DefId::TypeAlias(t.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;
                }
                ItemKind::Enum(e) => {
                    self.module_scope
                        .insert(e.name.clone(), DefId::Enum(e.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;

                    // Also add enum variants to the module scope
                    for variant in &e.variants {
                        self.module_scope
                            .insert(
                                variant.name.clone(),
                                DefId::EnumVariant {
                                    enum_name: e.name.clone(),
                                    variant_name: variant.name.clone(),
                                },
                            )
                            .map_err(|name| {
                                ParseError::new(
                                    ParseErrorKind::DuplicateDefinition(name.to_string()),
                                    variant.span,
                                )
                            })?;
                    }
                }
                ItemKind::Ability(a) => {
                    self.module_scope
                        .insert(a.name.clone(), DefId::Ability(a.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;
                }
                ItemKind::Trait(t) => {
                    self.module_scope
                        .insert(t.name.clone(), DefId::Trait(t.name.clone()))
                        .map_err(|name| {
                            ParseError::new(
                                ParseErrorKind::DuplicateDefinition(name.to_string()),
                                item.span,
                            )
                        })?;
                }
                ItemKind::Use(_) | ItemKind::Impl(_) => {
                    // Imports and impls are processed in separate passes
                }
            }
        }

        Ok(())
    }

    /// Process import statements.
    fn process_imports(&mut self, module: &Module) -> Result<(), ParseError> {
        for item in &module.items {
            if let ItemKind::Use(use_def) = &item.kind {
                // For now, we'll handle imports within the same module only
                // Full implementation would need a module registry
                match &use_def.kind {
                    ambient_engine::ast::UseKind::Module => {
                        // Import the module itself
                        // For now, this is a no-op since we don't have cross-module resolution yet
                        // TODO: implement once we have a module registry
                    }
                    ambient_engine::ast::UseKind::Items(items) => {
                        // Import specific items
                        // For now, check if they exist in the current module
                        for item_name in items {
                            // Check if the item exists in module scope
                            if let Some(def) = self.module_scope.get(item_name) {
                                // Add to import scope
                                self.import_scope
                                    .insert(item_name.clone(), def.clone())
                                    .map_err(|name| {
                                        ParseError::new(
                                            ParseErrorKind::DuplicateDefinition(name.to_string()),
                                            item.span,
                                        )
                                    })?;
                            }
                            // If not found, we'll let it pass for now
                            // Full implementation would check the module registry
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolve names within an item.
    fn resolve_item(&mut self, item: &Item) -> Result<(), ParseError> {
        match &item.kind {
            ItemKind::Function(f) => self.resolve_function(f),
            ItemKind::Const(c) => {
                // Resolve names in the constant value
                self.resolve_expr(&c.value)
            }
            ItemKind::TypeAlias(_)
            | ItemKind::Enum(_)
            | ItemKind::Ability(_)
            | ItemKind::Trait(_)
            | ItemKind::Use(_) => {
                // No name resolution needed for these yet
                Ok(())
            }
            ItemKind::Impl(impl_def) => {
                // Resolve impl method bodies
                for method in &impl_def.methods {
                    self.resolve_impl_method(method)?;
                }
                Ok(())
            }
        }
    }

    /// Resolve names within a function.
    fn resolve_function(&mut self, func: &FunctionDef) -> Result<(), ParseError> {
        // Push a new scope for the function
        self.push_scope();

        // Add parameters to the local scope
        for param in &func.params {
            self.add_local_binding(&param.name, param.id)?;
        }

        // Resolve the function body
        self.resolve_expr(&func.body)?;

        // Pop the function scope
        self.pop_scope();

        Ok(())
    }

    /// Resolve names within an impl method.
    fn resolve_impl_method(&mut self, method: &ImplMethod) -> Result<(), ParseError> {
        // Push a new scope for the method
        self.push_scope();

        // Add self parameter to the local scope
        self.add_local_binding(&Arc::from("self"), method.self_id)?;

        // Add other parameters to the local scope
        for param in &method.params {
            self.add_local_binding(&param.name, param.id)?;
        }

        // Resolve the method body
        self.resolve_expr(&method.body)?;

        // Pop the method scope
        self.pop_scope();

        Ok(())
    }

    /// Resolve names within an expression.
    #[allow(clippy::too_many_lines)]
    fn resolve_expr(&mut self, expr: &Expr) -> Result<(), ParseError> {
        match &expr.kind {
            ExprKind::Unit | ExprKind::Bool(_) | ExprKind::Number(_) | ExprKind::String(_) => {
                // Literals don't need resolution
                Ok(())
            }

            ExprKind::Local(_) => {
                // Already resolved by the parser/lowerer
                Ok(())
            }

            ExprKind::Name(qname) => {
                // Resolve the qualified name
                self.resolve_qualified_name(qname, expr.span)?;
                Ok(())
            }

            ExprKind::Tuple(elements) | ExprKind::List(elements) => {
                for elem in elements {
                    self.resolve_expr(elem)?;
                }
                Ok(())
            }

            ExprKind::TupleIndex(tuple, _) => self.resolve_expr(tuple),

            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.resolve_expr(value)?;
                }
                Ok(())
            }

            ExprKind::TypedRecord {
                type_name: _,
                fields,
            } => {
                // Don't resolve type_name here - it's a type, not a value.
                // Type alias resolution happens during type checking.
                for (_, value) in fields {
                    self.resolve_expr(value)?;
                }
                Ok(())
            }

            ExprKind::RecordField(record, _) => self.resolve_expr(record),

            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left)?;
                self.resolve_expr(right)?;
                Ok(())
            }

            ExprKind::Unary(_, operand) => self.resolve_expr(operand),

            ExprKind::If(cond, then_branch, else_branch) => {
                self.resolve_expr(cond)?;
                self.resolve_expr(then_branch)?;
                if let Some(else_br) = else_branch {
                    self.resolve_expr(else_br)?;
                }
                Ok(())
            }

            ExprKind::Match(scrutinee, arms) => {
                self.resolve_expr(scrutinee)?;
                for arm in arms {
                    // Push a new scope for the pattern bindings
                    self.push_scope();

                    // Add pattern bindings to the scope
                    self.collect_pattern_bindings(&arm.pattern)?;

                    // Resolve guard and body
                    if let Some(guard) = &arm.guard {
                        self.resolve_expr(guard)?;
                    }
                    self.resolve_expr(&arm.body)?;

                    // Pop the match arm scope
                    self.pop_scope();
                }
                Ok(())
            }

            ExprKind::Block(stmts, result) => {
                self.push_scope();

                for stmt in stmts {
                    self.resolve_stmt(stmt)?;
                }

                if let Some(res) = result {
                    self.resolve_expr(res)?;
                }

                self.pop_scope();
                Ok(())
            }

            ExprKind::Lambda(lambda) => {
                self.push_scope();

                // Add lambda parameters to the scope
                for param in &lambda.params {
                    self.add_local_binding(&param.name, param.id)?;
                }

                self.resolve_expr(&lambda.body)?;

                self.pop_scope();
                Ok(())
            }

            ExprKind::Call(callee, args) => {
                self.resolve_expr(callee)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                Ok(())
            }

            ExprKind::Perform(call) => {
                // Resolve the ability name
                self.resolve_qualified_name(&call.ability, call.span)?;

                // Resolve arguments
                for arg in &call.args {
                    self.resolve_expr(arg)?;
                }
                Ok(())
            }

            ExprKind::Handle(handle_expr) => {
                // Resolve the body
                self.resolve_expr(&handle_expr.body)?;

                // Resolve each handler
                for handler in &handle_expr.handlers {
                    self.push_scope();

                    // Add handler parameters to the scope
                    for param in &handler.params {
                        self.add_local_binding(&param.name, param.id)?;
                    }

                    // Resolve the ability name
                    self.resolve_qualified_name(&handler.ability, handler.span)?;

                    // Resolve handler body
                    self.resolve_expr(&handler.body)?;

                    self.pop_scope();
                }

                // Resolve else clause if present
                if let Some(else_clause) = &handle_expr.else_clause {
                    self.resolve_expr(else_clause)?;
                }

                Ok(())
            }

            ExprKind::Resume(value) => {
                // Resolve the value expression
                self.resolve_expr(value)
            }

            ExprKind::HandlerLiteral(handler_lit) => {
                // Resolve each method in the handler literal
                for method in &handler_lit.methods {
                    self.push_scope();

                    // Add method parameters to the scope
                    for param in &method.params {
                        self.add_local_binding(&param.name, param.id)?;
                    }

                    // Resolve method body
                    self.resolve_expr(&method.body)?;

                    self.pop_scope();
                }

                Ok(())
            }

            ExprKind::Sandbox(sandbox_expr) => {
                // Resolve the body of the sandbox
                // Ability names in allowed_abilities are validated during type checking
                self.resolve_expr(&sandbox_expr.body)
            }

            ExprKind::MethodCall { receiver, args, .. } => {
                // Resolve the receiver and all arguments
                self.resolve_expr(receiver)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                Ok(())
            }
        }
    }

    /// Resolve names within a statement.
    fn resolve_stmt(&mut self, stmt: &Stmt) -> Result<(), ParseError> {
        match &stmt.kind {
            StmtKind::Let(binding) => {
                // First resolve the initializer
                self.resolve_expr(&binding.init)?;

                // Then add the binding to the current scope
                self.add_local_binding(&binding.name, binding.id)?;

                Ok(())
            }
            StmtKind::Expr(expr) => self.resolve_expr(expr),
        }
    }

    /// Collect bindings from a pattern.
    fn collect_pattern_bindings(
        &mut self,
        pattern: &ambient_engine::ast::Pattern,
    ) -> Result<(), ParseError> {
        match &pattern.kind {
            ambient_engine::ast::PatternKind::Wildcard
            | ambient_engine::ast::PatternKind::Literal(_) => Ok(()),

            ambient_engine::ast::PatternKind::Binding(id, name) => {
                self.add_local_binding(name, *id)
            }

            ambient_engine::ast::PatternKind::Tuple(patterns) => {
                for pat in patterns {
                    self.collect_pattern_bindings(pat)?;
                }
                Ok(())
            }

            ambient_engine::ast::PatternKind::Record(fields) => {
                for (_, pat) in fields {
                    self.collect_pattern_bindings(pat)?;
                }
                Ok(())
            }

            ambient_engine::ast::PatternKind::Variant(_, payload) => {
                if let Some(pat) = payload {
                    self.collect_pattern_bindings(pat)?;
                }
                Ok(())
            }
        }
    }

    /// Resolve a qualified name to its definition.
    fn resolve_qualified_name(
        &self,
        qname: &QualifiedName,
        span: ambient_engine::ast::Span,
    ) -> Result<DefId, ParseError> {
        // For now, we only handle simple unqualified names
        // Full implementation would handle module paths
        if !qname.path.is_empty() {
            // TODO: handle qualified names from other modules
            return Ok(DefId::Function(qname.name.clone()));
        }

        // Check local scopes (innermost to outermost)
        for scope in self.local_scopes.iter().rev() {
            if let Some(def) = scope.get(&qname.name) {
                return Ok(def.clone());
            }
        }

        // Check import scope
        if let Some(def) = self.import_scope.get(&qname.name) {
            return Ok(def.clone());
        }

        // Check module scope
        if let Some(def) = self.module_scope.get(&qname.name) {
            return Ok(def.clone());
        }

        // Name not found
        Err(ParseError::new(
            ParseErrorKind::UndefinedName(qname.name.to_string()),
            span,
        ))
    }

    /// Push a new local scope onto the stack.
    fn push_scope(&mut self) {
        self.local_scopes.push(Scope::new());
    }

    /// Pop the current local scope from the stack.
    fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }

    /// Add a local binding to the current scope.
    fn add_local_binding(&mut self, name: &Arc<str>, id: BindingId) -> Result<(), ParseError> {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope
                .insert(Arc::clone(name), DefId::Local(id))
                .map_err(|_| {
                    ParseError::new(
                        ParseErrorKind::DuplicateDefinition(name.to_string()),
                        ambient_engine::ast::Span::default(),
                    )
                })?;
        }
        Ok(())
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn test_resolve_simple_function() {
        let source = "fn add(x: number, y: number): number { x + y }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }

    #[test]
    fn test_resolve_function_call() {
        let source = "fn helper(x: number): number { x + 1 } fn run(): number { helper(42) }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }

    #[test]
    fn test_undefined_name() {
        let source = "fn foo(): number { undefined_var }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_err(), "should fail with undefined name");
    }

    #[test]
    fn test_duplicate_definition() {
        let source = "fn foo(): number { 1 } fn foo(): number { 2 }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_err(), "should fail with duplicate definition");
    }

    #[test]
    fn test_let_binding_scope() {
        let source = "fn foo(): number { let x = 1; let y = x + 1; y }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }

    #[test]
    fn test_shadowing_allowed() {
        let source = "fn foo(x: number): number { let x = x + 1; x }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        // Shadowing should be allowed (parameter x is shadowed by let x)
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }

    #[test]
    fn test_match_pattern_bindings() {
        let source = "enum Option { Some(number), None } fn unwrap(opt: Option): number { match opt { Some(x) => x, None => 0 } }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }

    #[test]
    fn test_enum_variant_resolution() {
        let source = "enum Option { Some(number), None } fn make_some(): Option { Some(42) }";
        let module = parse(source).expect("parse error");

        let mut resolver = Resolver::new();
        let result = resolver.resolve_module(&module);
        assert!(result.is_ok(), "resolution failed: {:?}", result.err());
    }
}
