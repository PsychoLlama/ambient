//! Semantic token support for the LSP.
//!
//! Provides semantic highlighting information to editors, allowing them to
//! color code based on semantic meaning (function calls, variables, types, etc.)
//! rather than just syntax.

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

use ambient_engine::ast::{
    AbilityCall, Expr, ExprKind, HandlerLiteralExpr, Item, ItemKind, Lambda, MatchArm, Module,
    Pattern, PatternKind, SandboxExpr, Span, Stmt, StmtKind,
};

use crate::documents::Document;

/// Token types we support.
/// The order here defines the token type index in the legend.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::FUNCTION,    // 0 - function definitions and calls
    SemanticTokenType::VARIABLE,    // 1 - local variables
    SemanticTokenType::PARAMETER,   // 2 - function parameters
    SemanticTokenType::TYPE,        // 3 - type names
    SemanticTokenType::ENUM,        // 4 - enum types
    SemanticTokenType::ENUM_MEMBER, // 5 - enum variants
    SemanticTokenType::PROPERTY,    // 6 - record fields
    SemanticTokenType::STRING,      // 7 - string literals
    SemanticTokenType::NUMBER,      // 8 - number literals
    SemanticTokenType::KEYWORD,     // 9 - keywords
    SemanticTokenType::OPERATOR,    // 10 - operators
    SemanticTokenType::INTERFACE,   // 11 - abilities
    SemanticTokenType::METHOD,      // 12 - ability methods
    SemanticTokenType::NAMESPACE,   // 13 - module paths
];

/// Token modifiers we support.
pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,     // 0 - definition site
    SemanticTokenModifier::DEFINITION,      // 1 - definition
    SemanticTokenModifier::READONLY,        // 2 - constants
    SemanticTokenModifier::STATIC,          // 3 - static/module-level
    SemanticTokenModifier::DEFAULT_LIBRARY, // 4 - core library
];

/// Create the semantic tokens legend.
#[must_use]
pub fn create_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

/// Index constants for token types.
///
/// All LSP-defined token types are included for protocol completeness,
/// even if not all are currently emitted.
#[allow(dead_code)]
mod token_type {
    pub const FUNCTION: u32 = 0;
    pub const VARIABLE: u32 = 1;
    pub const PARAMETER: u32 = 2;
    pub const TYPE: u32 = 3;
    pub const ENUM: u32 = 4;
    pub const ENUM_MEMBER: u32 = 5;
    pub const PROPERTY: u32 = 6;
    pub const STRING: u32 = 7;
    pub const NUMBER: u32 = 8;
    pub const KEYWORD: u32 = 9;
    pub const OPERATOR: u32 = 10;
    pub const INTERFACE: u32 = 11;
    pub const METHOD: u32 = 12;
    pub const NAMESPACE: u32 = 13;
}

/// Index constants for token modifiers (as bit flags).
///
/// All LSP-defined modifiers are included for protocol completeness,
/// even if not all are currently used.
#[allow(dead_code)]
mod token_modifier {
    pub const DECLARATION: u32 = 1 << 0;
    pub const DEFINITION: u32 = 1 << 1;
    pub const READONLY: u32 = 1 << 2;
    pub const STATIC: u32 = 1 << 3;
    pub const DEFAULT_LIBRARY: u32 = 1 << 4;
}

/// A raw token before delta encoding.
#[derive(Debug, Clone)]
struct RawToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    token_modifiers: u32,
}

/// Extract semantic tokens from a module.
#[must_use]
pub fn extract_semantic_tokens(module: &Module, doc: &Document) -> Vec<SemanticToken> {
    let mut collector = TokenCollector::new(doc);
    collector.visit_module(module);
    collector.into_semantic_tokens()
}

/// Helper to safely convert string length to u32.
fn str_len_u32(s: &str) -> u32 {
    u32::try_from(s.len()).unwrap_or(u32::MAX)
}

/// Collects tokens while walking the AST.
struct TokenCollector<'a> {
    doc: &'a Document,
    tokens: Vec<RawToken>,
}

impl<'a> TokenCollector<'a> {
    fn new(doc: &'a Document) -> Self {
        Self {
            doc,
            tokens: Vec::new(),
        }
    }

    /// Convert collected tokens to delta-encoded semantic tokens.
    fn into_semantic_tokens(mut self) -> Vec<SemanticToken> {
        // Sort by position
        self.tokens.sort_by_key(|t| (t.line, t.start_char));

        // Delta encode
        let mut result = Vec::with_capacity(self.tokens.len());
        let mut prev_line = 0u32;
        let mut prev_start = 0u32;

        for token in self.tokens {
            let delta_line = token.line - prev_line;
            let delta_start = if delta_line == 0 {
                token.start_char - prev_start
            } else {
                token.start_char
            };

            result.push(SemanticToken {
                delta_line,
                delta_start,
                length: token.length,
                token_type: token.token_type,
                token_modifiers_bitset: token.token_modifiers,
            });

            prev_line = token.line;
            prev_start = token.start_char;
        }

        result
    }

    /// Add a token at the given byte offsets.
    fn add_token(&mut self, start: u32, end: u32, token_type: u32, token_modifiers: u32) {
        let (line, start_char) = self.doc.offset_to_position(start as usize);
        let length = end.saturating_sub(start);
        if length > 0 {
            self.tokens.push(RawToken {
                line,
                start_char,
                length,
                token_type,
                token_modifiers,
            });
        }
    }

    /// Emit a `TYPE` declaration token for a type-like definition name (a
    /// `struct` or a `type` alias).
    fn add_type_decl_token(&mut self, name_span: Span) {
        self.add_token(
            name_span.start,
            name_span.end,
            token_type::TYPE,
            token_modifier::DECLARATION,
        );
    }

    fn visit_module(&mut self, module: &Module) {
        for item in &module.items {
            self.visit_item(item);
        }
    }

    fn visit_item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Function(f) => {
                // Function name at definition
                self.add_token(
                    f.name_span.start,
                    f.name_span.end,
                    token_type::FUNCTION,
                    token_modifier::DECLARATION,
                );

                // Parameters
                for param in &f.params {
                    self.add_token(
                        param.span.start,
                        param.span.start + str_len_u32(&param.name),
                        token_type::PARAMETER,
                        token_modifier::DECLARATION,
                    );
                }

                // Function body
                self.visit_expr(&f.body);
            }
            ItemKind::Const(c) => {
                // Constant name
                self.add_token(
                    c.name_span.start,
                    c.name_span.end,
                    token_type::VARIABLE,
                    token_modifier::DECLARATION | token_modifier::READONLY,
                );
                // Value
                self.visit_expr(&c.value);
            }
            // A struct or type-alias name both highlight as a type declaration.
            ItemKind::Struct(s) => self.add_type_decl_token(s.name_span),
            ItemKind::TypeAlias(t) => self.add_type_decl_token(t.name_span),
            ItemKind::Enum(e) => {
                // Enum name at definition
                self.add_token(
                    e.name_span.start,
                    e.name_span.end,
                    token_type::ENUM,
                    token_modifier::DECLARATION,
                );
                // Variants
                for variant in &e.variants {
                    self.add_token(
                        variant.span.start,
                        variant.span.start + str_len_u32(&variant.name),
                        token_type::ENUM_MEMBER,
                        token_modifier::DECLARATION,
                    );
                }
            }
            ItemKind::Ability(a) => {
                // Ability name at definition
                self.add_token(
                    a.name_span.start,
                    a.name_span.end,
                    token_type::INTERFACE,
                    token_modifier::DECLARATION,
                );
                // Methods
                for method in &a.methods {
                    self.add_token(
                        method.span.start,
                        method.span.start + str_len_u32(&method.name),
                        token_type::METHOD,
                        token_modifier::DECLARATION,
                    );
                }
            }
            ItemKind::Use(_) => {
                // Use statements - could highlight the path segments
            }
            ItemKind::Trait(t) => {
                // Trait name at definition
                self.add_token(
                    t.name_span.start,
                    t.name_span.end,
                    token_type::INTERFACE,
                    token_modifier::DECLARATION,
                );
                // Methods
                for method in &t.methods {
                    self.add_token(
                        method.name_span.start,
                        method.name_span.end,
                        token_type::METHOD,
                        token_modifier::DECLARATION,
                    );
                }
            }
            ItemKind::Impl(i) => {
                // Visit method bodies
                for method in &i.methods {
                    self.add_token(
                        method.name_span.start,
                        method.name_span.end,
                        token_type::METHOD,
                        token_modifier::DECLARATION,
                    );
                    self.visit_expr(&method.body);
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Unit => {}
            ExprKind::Bool(_) => {
                self.add_token(expr.span.start, expr.span.end, token_type::KEYWORD, 0);
            }
            ExprKind::Number(_) => {
                self.add_token(expr.span.start, expr.span.end, token_type::NUMBER, 0);
            }
            ExprKind::String(_) => {
                self.add_token(expr.span.start, expr.span.end, token_type::STRING, 0);
            }
            ExprKind::Local(_) => {
                self.add_token(expr.span.start, expr.span.end, token_type::VARIABLE, 0);
            }
            ExprKind::Name(qname) => {
                self.visit_name_expr(expr, qname);
            }
            ExprKind::Tuple(elements) | ExprKind::List(elements) => {
                for e in elements {
                    self.visit_expr(e);
                }
            }
            ExprKind::TupleIndex(base, _) | ExprKind::RecordField(base, _) => {
                self.visit_expr(base);
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.visit_expr(value);
                }
            }
            ExprKind::TypedRecord { fields, .. } => {
                // The type name could be highlighted as a type, but for now
                // just visit the field values
                for (_, value) in fields {
                    self.visit_expr(value);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary(_, operand) => {
                self.visit_expr(operand);
            }
            ExprKind::If(cond, then_branch, else_branch) => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(else_b) = else_branch {
                    self.visit_expr(else_b);
                }
            }
            ExprKind::Match(scrutinee, arms) => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_match_arm(arm);
                }
            }
            ExprKind::Block(stmts, tail) => {
                for stmt in stmts {
                    self.visit_stmt(stmt);
                }
                if let Some(tail) = tail {
                    self.visit_expr(tail);
                }
            }
            ExprKind::Lambda(lambda) => self.visit_lambda(lambda),
            ExprKind::Call(callee, args) => {
                self.visit_expr(callee);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            ExprKind::Perform(call) => self.visit_ability_call(call),
            ExprKind::Handle(h) => self.visit_handle_expr(h),
            ExprKind::Resume(value) => self.visit_expr(value),
            ExprKind::HandlerLiteral(h) => self.visit_handler_literal(h),
            ExprKind::Sandbox(s) => self.visit_sandbox(s),
            ExprKind::MethodCall {
                receiver,
                method_span,
                args,
                ..
            } => {
                // Highlight method name
                self.add_token(method_span.start, method_span.end, token_type::METHOD, 0);
                self.visit_expr(receiver);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
        }
    }

    fn visit_name_expr(&mut self, expr: &Expr, qname: &ambient_engine::ast::QualifiedName) {
        // Could be function, constant, enum, etc.
        // For now, treat as function. We'd need type info to distinguish.
        //
        // Positions come from the parser's recorded spans; the previous
        // arithmetic summed segment lengths plus one separator byte, but
        // the real separator is `::` (two bytes), so every qualified
        // name's highlighting drifted one byte per segment.
        for span in &qname.path_spans {
            self.add_token(span.start, span.end, token_type::NAMESPACE, 0);
        }
        match qname.name_span {
            Some(span) => self.add_token(span.start, span.end, token_type::FUNCTION, 0),
            // Bare names cover the whole expression span.
            None if qname.path.is_empty() => {
                self.add_token(expr.span.start, expr.span.end, token_type::FUNCTION, 0);
            }
            // Qualified with no recorded span: skip rather than guess.
            None => {}
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(binding) => {
                self.add_token(
                    binding.name_span.start,
                    binding.name_span.end,
                    token_type::VARIABLE,
                    token_modifier::DECLARATION,
                );
                self.visit_expr(&binding.init);
            }
            StmtKind::Expr(expr) => {
                self.visit_expr(expr);
            }
            StmtKind::Use(_) => {
                // Import paths get their coloring from the syntactic
                // highlighter; nothing semantic to add.
            }
        }
    }

    fn visit_match_arm(&mut self, arm: &MatchArm) {
        self.visit_pattern(&arm.pattern);
        self.visit_expr(&arm.body);
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
            PatternKind::Binding(_, name) => {
                let name_end = pattern.span.start + str_len_u32(name);
                self.add_token(
                    pattern.span.start,
                    name_end,
                    token_type::VARIABLE,
                    token_modifier::DECLARATION,
                );
            }
            PatternKind::Variant(_, inner) => {
                if let Some(inner_pattern) = inner {
                    self.visit_pattern(inner_pattern);
                }
            }
            PatternKind::Tuple(elements) => {
                for elem in elements {
                    self.visit_pattern(elem);
                }
            }
            PatternKind::Record(fields) => {
                for (_, pat) in fields {
                    self.visit_pattern(pat);
                }
            }
        }
    }

    fn visit_lambda(&mut self, lambda: &Lambda) {
        for param in &lambda.params {
            self.add_token(
                param.span.start,
                param.span.start + str_len_u32(&param.name),
                token_type::PARAMETER,
                token_modifier::DECLARATION,
            );
        }
        self.visit_expr(&lambda.body);
    }

    fn visit_ability_call(&mut self, call: &AbilityCall) {
        for arg in &call.args {
            self.visit_expr(arg);
        }
    }

    fn visit_handle_expr(&mut self, handle: &ambient_engine::ast::HandleExpr) {
        // Each handler is an ordinary expression (a literal or a value);
        // visiting it recurses into HandlerLiteral below.
        for handler in &handle.handlers {
            self.visit_expr(handler);
        }
        self.visit_expr(&handle.body);
        if let Some(else_clause) = &handle.else_clause {
            self.visit_expr(else_clause);
        }
    }

    fn visit_handler_literal(&mut self, handler: &HandlerLiteralExpr) {
        for method in &handler.methods {
            // Highlight the qualified `Ability::method` head: the ability as a
            // type, the method as an ability method.
            if let Some(ability_span) = method.ability.name_span {
                self.add_token(ability_span.start, ability_span.end, token_type::TYPE, 0);
            }
            self.add_token(
                method.method_span.start,
                method.method_span.end,
                token_type::METHOD,
                0,
            );
            self.visit_expr(&method.body);
        }
    }

    fn visit_sandbox(&mut self, sandbox: &SandboxExpr) {
        self.visit_expr(&sandbox.body);
    }
}
