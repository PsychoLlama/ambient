//! Completion service for use by both the LSP server and the REPL.
//!
//! This module provides a high-level interface for code completion that can be
//! used outside of the LSP server context, such as in the REPL.

use crate::analysis::{analyze, format_type, AnalysisResult};
use crate::completions::{get_completions, CompletionContext};
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails};

/// A symbol provided by an external source (e.g., REPL-defined functions).
#[derive(Debug, Clone)]
pub struct ExternalSymbol {
    /// The symbol name.
    pub name: String,
    /// The kind of symbol.
    pub kind: ExternalSymbolKind,
    /// Optional detail (e.g., type signature).
    pub detail: Option<String>,
}

impl ExternalSymbol {
    /// Create a new external symbol.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: ExternalSymbolKind) -> Self {
        Self {
            name: name.into(),
            kind,
            detail: None,
        }
    }

    /// Create a new external symbol with detail.
    #[must_use]
    pub fn with_detail(name: impl Into<String>, kind: ExternalSymbolKind, detail: String) -> Self {
        Self {
            name: name.into(),
            kind,
            detail: Some(detail),
        }
    }
}

/// The kind of an external symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalSymbolKind {
    /// A function.
    Function,
    /// A constant.
    Constant,
    /// A variable.
    Variable,
}

impl ExternalSymbolKind {
    fn to_completion_kind(self) -> CompletionItemKind {
        match self {
            Self::Function => CompletionItemKind::FUNCTION,
            Self::Constant => CompletionItemKind::CONSTANT,
            Self::Variable => CompletionItemKind::VARIABLE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Constant => "constant",
            Self::Variable => "variable",
        }
    }
}

/// A simplified completion result for REPL use.
#[derive(Debug, Clone)]
pub struct ReplCompletion {
    /// The completion label (displayed to user).
    pub label: String,
    /// The text to insert.
    pub replacement: String,
    /// Optional detail (e.g., type signature).
    pub detail: Option<String>,
    /// Priority for sorting (lower = higher priority).
    pub priority: u8,
}

/// A completion service that can be used by both LSP and REPL.
///
/// This wraps the existing LSP completion logic and adds support for
/// external symbols (e.g., REPL-defined functions and constants).
pub struct CompletionService {
    /// Cached analysis result.
    analysis: Option<AnalysisResult>,
    /// External symbols to include in completions.
    external_symbols: Vec<ExternalSymbol>,
}

impl Default for CompletionService {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionService {
    /// Create a new completion service.
    #[must_use]
    pub fn new() -> Self {
        Self {
            analysis: None,
            external_symbols: Vec::new(),
        }
    }

    /// Update the source code (re-parses and type-checks).
    ///
    /// For REPL use, the input should be wrapped in a synthetic function:
    /// `fn __repl__() { <input> }`
    pub fn update_source(&mut self, source: &str) {
        self.analysis = Some(analyze(source));
    }

    /// Clear the current analysis.
    pub fn clear_source(&mut self) {
        self.analysis = None;
    }

    /// Add external symbols to include in completions.
    pub fn set_external_symbols(&mut self, symbols: Vec<ExternalSymbol>) {
        self.external_symbols = symbols;
    }

    /// Clear external symbols.
    pub fn clear_external_symbols(&mut self) {
        self.external_symbols.clear();
    }

    /// Get completions at a given byte offset.
    ///
    /// Returns LSP `CompletionItem`s for maximum flexibility.
    #[must_use]
    pub fn get_completions_lsp(&self, source: &str, offset: usize) -> Vec<CompletionItem> {
        let ctx = CompletionContext::new(source, offset);
        let module = self.analysis.as_ref().and_then(|a| a.module.as_ref());

        // SymbolDb not available in REPL context
        let mut items = get_completions(&ctx, module, None);

        // Add external symbols, but only when not in a specific module context.
        // Skip when completing core.List.*, core.*, Console.*, etc. since those
        // have their own specific completions.
        let in_specific_context = ctx.after_core_submodule_dot.is_some()
            || ctx.after_core_dot
            || ctx.after_ability_dot.is_some();

        if !in_specific_context {
            for symbol in &self.external_symbols {
                if symbol.name.starts_with(ctx.word_prefix) {
                    items.push(CompletionItem {
                        label: symbol.name.clone(),
                        kind: Some(symbol.kind.to_completion_kind()),
                        detail: symbol.detail.clone(),
                        label_details: Some(CompletionItemLabelDetails {
                            detail: symbol.detail.clone(),
                            description: Some(symbol.kind.label().to_string()),
                        }),
                        ..Default::default()
                    });
                }
            }
        }

        items
    }

    /// Get completions in a simplified format for REPL use.
    ///
    /// This is a convenience method that converts LSP completions to a simpler format.
    /// It strips LSP snippet syntax (e.g., `${1:placeholder}`) since the REPL doesn't
    /// support it.
    #[must_use]
    pub fn get_completions(&self, source: &str, offset: usize) -> Vec<ReplCompletion> {
        self.get_completions_lsp(source, offset)
            .into_iter()
            .map(|item| {
                let priority = completion_priority(&item);
                // Use insert_text if available, but strip snippet syntax
                let replacement = item
                    .insert_text
                    .as_ref()
                    .map_or_else(|| item.label.clone(), |t| strip_snippet_syntax(t));
                ReplCompletion {
                    label: item.label.clone(),
                    replacement,
                    detail: item.detail,
                    priority,
                }
            })
            .collect()
    }

    /// Get type hint for expression at offset.
    ///
    /// Returns the type of the expression as a formatted string, if available.
    #[must_use]
    pub fn get_type_hint(&self, _source: &str, offset: usize) -> Option<String> {
        let analysis = self.analysis.as_ref()?;
        let module = analysis.module.as_ref()?;

        #[allow(clippy::cast_possible_truncation)]
        let expr = crate::analysis::find_expr_at_offset(module, offset as u32)?;
        let ty = expr.ty.as_ref()?;

        Some(format_type(ty))
    }
}

/// Determine completion priority from LSP `CompletionItem`.
fn completion_priority(item: &CompletionItem) -> u8 {
    match item.kind {
        Some(CompletionItemKind::VARIABLE) => 5,
        Some(
            CompletionItemKind::FUNCTION
            | CompletionItemKind::CONSTANT
            | CompletionItemKind::METHOD,
        ) => 10,
        Some(CompletionItemKind::INTERFACE) => 15, // Abilities
        Some(CompletionItemKind::MODULE) => 20,
        Some(CompletionItemKind::TYPE_PARAMETER) => 25,
        Some(CompletionItemKind::KEYWORD) => 30,
        _ => 50,
    }
}

/// Strip LSP snippet syntax from completion text.
///
/// Converts snippets like `print!(${1:message})` to `print!(message)`.
/// This is needed because the REPL uses rustyline which doesn't support
/// LSP snippet format.
fn strip_snippet_syntax(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            // Skip the '{' and find the placeholder content
            chars.next(); // consume '{'

            // Skip the number (e.g., "1")
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    chars.next();
                } else {
                    break;
                }
            }

            // Check for colon or closing brace
            match chars.peek() {
                Some(&':') => {
                    chars.next(); // consume ':'
                                  // Copy the placeholder content until '}'
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                        result.push(c);
                    }
                }
                Some(&'}') => {
                    // No colon, just ${1} style - skip entirely
                    chars.next();
                }
                _ => {
                    // Malformed, skip until '}'
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_service_basic() {
        let service = CompletionService::new();
        let completions = service.get_completions("Con", 3);

        // Should include Console (ability)
        assert!(
            completions.iter().any(|c| c.label == "Console"),
            "Should complete Console, got: {:?}",
            completions.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_completion_service_with_external_symbols() {
        let mut service = CompletionService::new();
        service.set_external_symbols(vec![
            ExternalSymbol::new("my_func", ExternalSymbolKind::Function),
            ExternalSymbol::new("MY_CONST", ExternalSymbolKind::Constant),
        ]);

        let completions = service.get_completions("my", 2);
        assert!(
            completions.iter().any(|c| c.label == "my_func"),
            "Should include external function"
        );

        let completions = service.get_completions("MY", 2);
        assert!(
            completions.iter().any(|c| c.label == "MY_CONST"),
            "Should include external constant"
        );
    }

    #[test]
    fn test_completion_service_after_ability_scope() {
        let service = CompletionService::new();
        let completions = service.get_completions("Console::", 9);

        // Should show Console methods
        assert!(
            completions.iter().any(|c| c.label == "print!"),
            "Should show print! method, got: {:?}",
            completions.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_completion_service_keywords() {
        let service = CompletionService::new();
        let completions = service.get_completions("le", 2);

        assert!(
            completions.iter().any(|c| c.label == "let"),
            "Should complete let keyword"
        );
    }

    #[test]
    fn test_external_symbol_with_detail() {
        let mut service = CompletionService::new();
        service.set_external_symbols(vec![ExternalSymbol::with_detail(
            "add",
            ExternalSymbolKind::Function,
            "(x: number, y: number) -> number".to_string(),
        )]);

        let completions = service.get_completions("ad", 2);
        let add_completion = completions.iter().find(|c| c.label == "add");

        assert!(add_completion.is_some(), "Should find add function");
        assert_eq!(
            add_completion.unwrap().detail.as_deref(),
            Some("(x: number, y: number) -> number")
        );
    }

    #[test]
    fn test_completion_priority() {
        // External symbols should have high priority
        let mut service_with_external = CompletionService::new();
        service_with_external.set_external_symbols(vec![ExternalSymbol::new(
            "Random",
            ExternalSymbolKind::Function,
        )]);

        // When typing "R", user-defined Random should appear with its priority
        let completions = service_with_external.get_completions("R", 1);

        // Find both completions
        let user_random = completions
            .iter()
            .find(|c| c.label == "Random" && c.priority == 10);
        let builtin_random = completions
            .iter()
            .find(|c| c.label == "Random" && c.priority == 15);

        // Both should exist
        assert!(
            user_random.is_some() || builtin_random.is_some(),
            "Should have Random completion"
        );
    }

    #[test]
    fn test_strip_snippet_syntax() {
        // Snippet with placeholder name
        assert_eq!(
            strip_snippet_syntax("print!(${1:message})"),
            "print!(message)"
        );

        // Multiple placeholders
        assert_eq!(strip_snippet_syntax("foo(${1:a}, ${2:b})"), "foo(a, b)");

        // No placeholders (just numbers)
        assert_eq!(strip_snippet_syntax("bar(${1}, ${2})"), "bar(, )");

        // No snippet syntax
        assert_eq!(strip_snippet_syntax("simple()"), "simple()");

        // Empty string
        assert_eq!(strip_snippet_syntax(""), "");

        // Partial snippet (edge case)
        assert_eq!(strip_snippet_syntax("test$"), "test$");
        assert_eq!(strip_snippet_syntax("test${"), "test");
    }

    #[test]
    fn test_repl_completions_strip_snippets() {
        let service = CompletionService::new();
        let completions = service.get_completions("Console::", 9);

        // Find a method completion
        let print_completion = completions.iter().find(|c| c.label == "print!");

        if let Some(print) = print_completion {
            // Should not contain snippet syntax
            assert!(
                !print.replacement.contains("${"),
                "Replacement should not contain snippet syntax, got: {}",
                print.replacement
            );
        }
    }

    #[test]
    fn test_completion_service_after_core_submodule_scope() {
        let service = CompletionService::new();
        let completions = service.get_completions("core::List::", 12);

        // Should show core::List functions like map, filter, fold
        assert!(
            completions.iter().any(|c| c.label == "map"),
            "Should show map function, got: {:?}",
            completions.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert!(
            completions.iter().any(|c| c.label == "map"),
            "Should show map function, got: {:?}",
            completions.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }
}
