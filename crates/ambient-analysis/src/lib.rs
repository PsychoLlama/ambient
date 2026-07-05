//! Shared analysis layer for the Ambient language.
//!
//! One parse → check → diagnose pipeline, consumed by both the CLI
//! (`ambient check`) and the language server. Anything that decides *what
//! is an error* lives here, once — the frontends only render. If a
//! behavior needs to differ between the compiler and the editor, the
//! difference must be expressed in this crate where it is visible and
//! testable, not re-implemented downstream.
//!
//! ```text
//! source ──parse_recovering──▶ partial AST + parse errors
//!            │
//!            ▼
//!    check_module_with_registry (engine)
//!            │
//!            ▼
//!    AnalysisResult ──diagnostics()──▶ ordered, deduplicated diagnostics
//! ```
//!
//! Files mid-edit are the normal case for the LSP, so parsing always
//! recovers: a broken item is dropped, everything else is analyzed. Type
//! errors are still computed on the partial module (the typed AST feeds
//! hover/completion), but [`AnalysisResult::diagnostics`] suppresses them
//! while parse errors exist — checking a module with missing items
//! produces cascading nonsense, and the CLI's behavior (stop after parse
//! errors) is the sane baseline both frontends share.

#![warn(clippy::print_stdout, clippy::print_stderr)]
#![deny(
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::complexity,
    clippy::correctness,
    clippy::suspicious,
    clippy::unwrap_used,
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![allow(clippy::missing_errors_doc)]

pub mod package;
pub mod queries;

use ambient_engine::ability_resolver::AbilityResolver;
use ambient_engine::ast::{Module, Span};
use ambient_engine::infer::{
    BoxedTypeError, check_module_with_registry, check_module_with_registry_and_resolver,
};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_parser::{ParseError, parse_recovering};

/// The severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A rendered diagnostic: everything a frontend needs to display it.
///
/// This is the *shared* currency between the CLI and the LSP. Both render
/// exactly this — same message text, same span, same note — so a
/// discrepancy between `ambient check` and the editor is a bug in this
/// crate, not a divergence between two implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte span in the source.
    pub span: Span,
    /// The primary message.
    pub message: String,
    /// Optional note displayed alongside the message.
    pub note: Option<String>,
    /// Severity level.
    pub severity: Severity,
}

/// The result of analyzing one module's source.
#[derive(Debug)]
pub struct AnalysisResult {
    /// All parse and lowering errors, in source order.
    pub parse_errors: Vec<ParseError>,
    /// Type errors from checking the (possibly partial) module.
    pub type_errors: Vec<BoxedTypeError>,
    /// The typed AST. Always present — files that fail to lex entirely
    /// yield an empty module. Items that failed to parse are missing.
    pub module: Module,
}

impl AnalysisResult {
    /// Whether any reportable problem was found.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.parse_errors.is_empty() || !self.type_errors.is_empty()
    }

    /// The diagnostics both frontends report, in source order.
    ///
    /// Policy (shared by `ambient check` and the editor): while parse
    /// errors exist, type errors are suppressed — a module with items
    /// missing produces cascading, misleading type errors. The typed
    /// module is still available for IDE features; it is only the
    /// *reporting* that stays conservative.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out: Vec<Diagnostic> = self
            .parse_errors
            .iter()
            .map(|e| Diagnostic {
                span: e.span,
                message: e.kind.to_string(),
                note: e.context.clone(),
                severity: Severity::Error,
            })
            .collect();

        if out.is_empty() {
            out.extend(self.type_errors.iter().map(|e| Diagnostic {
                span: Span::new(e.span.0, e.span.1),
                message: e.kind.to_string(),
                note: e.context.clone(),
                severity: Severity::Error,
            }));
        }

        out.sort_by_key(|d| (d.span.start, d.span.end));
        out
    }
}

/// Analyze a single module without package context.
///
/// The module is checked as a package root against the core and platform
/// declaration modules, exactly like `ambient check` on a bare file.
#[must_use]
pub fn analyze(source: &str) -> AnalysisResult {
    analyze_with_registry(source, None, None)
}

/// A registry seeded with the core library and the `platform` declaration
/// module — the context every Ambient module is checked in, package or not.
#[must_use]
pub fn core_platform_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();

    let _ = ambient_engine::core_library::register_core_modules(&mut registry, |source| {
        ambient_parser::parse(source).map_err(|e| e.to_string())
    });

    let _ = ambient_engine::core_library::register_declaration_module(
        &mut registry,
        &["platform"],
        ambient_platform::ABILITY_DECLARATIONS,
        |source| ambient_parser::parse(source).map_err(|e| e.to_string()),
    );

    registry
}

/// Analyze a module with cross-module support.
///
/// When a registry is provided it must contain this module (registered
/// under `module_path`) alongside the rest of the package plus the
/// `core`/`platform` declaration modules — [`package::AnalysisPackage`]
/// builds exactly that.
#[must_use]
pub fn analyze_with_registry(
    source: &str,
    module_path: Option<&ModulePath>,
    registry: Option<&ModuleRegistry>,
) -> AnalysisResult {
    analyze_with_registry_and_resolver(source, module_path, registry, None)
}

/// Analyze with cross-module support and a custom ability resolver
/// (respecting a package's `[host].abilities` configuration).
#[must_use]
pub fn analyze_with_registry_and_resolver(
    source: &str,
    module_path: Option<&ModulePath>,
    registry: Option<&ModuleRegistry>,
    resolver: Option<AbilityResolver>,
) -> AnalysisResult {
    let recovered = parse_recovering(source);
    let parse_errors = recovered.errors;
    let module = recovered.module;

    // Type check the (possibly partial) module. The registry carries the
    // `platform` declaration module, so `platform::…` abilities resolve
    // through engine registry seeding — no embedder resolver needed. When
    // no package registry is supplied, the module checks as a package
    // root against a core+platform registry, so single-file analysis sees
    // the same world a package build does.
    let fallback_path;
    let fallback_registry;
    let (path, reg) = if let (Some(path), Some(reg)) = (module_path, registry) {
        (path, reg)
    } else {
        fallback_path = ModulePath::root();
        fallback_registry = {
            let mut registry = core_platform_registry();
            registry.register(&fallback_path, std::sync::Arc::new(module.clone()));
            registry
        };
        (&fallback_path, &fallback_registry)
    };
    let check_result = match resolver {
        Some(res) => check_module_with_registry_and_resolver(module, path, reg, res),
        None => check_module_with_registry(module, path, reg),
    };

    AnalysisResult {
        parse_errors,
        type_errors: check_result.errors,
        module: check_result.module,
    }
}

/// An ability resolver with the platform bindings interface registered
/// under the `platform` namespace, mirroring how the CLI checks code.
#[must_use]
pub fn platform_prelude_resolver() -> AbilityResolver {
    let mut resolver = ambient_engine::ability_resolver::core_abilities();
    if let Ok(mut module) = ambient_parser::parse(ambient_platform::ABILITY_DECLARATIONS) {
        let (abilities, _errors) = ambient_engine::infer::resolve_ability_declarations(&mut module);
        for ability in abilities {
            resolver.register_dynamic_in_namespace("platform", (*ability).clone());
        }
    }
    resolver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_source_has_no_diagnostics() {
        let result = analyze("fn add(x: number, y: number): number { x + y }");
        assert!(!result.has_errors());
        assert_eq!(result.module.items.len(), 1);
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn type_error_is_reported() {
        let result = analyze("fn bad(): string { 42 }");
        assert!(result.has_errors());
        assert_eq!(result.parse_errors.len(), 0);
        assert!(!result.type_errors.is_empty());
        assert!(!result.diagnostics().is_empty());
    }

    #[test]
    fn parse_error_still_yields_partial_module() {
        let result = analyze("fn ok(): number { 1 }\nfn broken(\n");
        assert!(!result.parse_errors.is_empty());
        assert_eq!(result.module.items.len(), 1);
    }

    #[test]
    fn type_errors_suppressed_while_parse_errors_exist() {
        // `bad` has a type error, `broken` a parse error. Reporting shows
        // only the parse error (CLI parity); the type error is still
        // computed for IDE consumers.
        let result = analyze("fn bad(): string { 42 }\nfn broken(\n");
        let diagnostics = result.diagnostics();
        assert!(!result.parse_errors.is_empty());
        assert!(!result.type_errors.is_empty());
        assert_eq!(diagnostics.len(), result.parse_errors.len());
    }

    #[test]
    fn lexer_error_yields_empty_module() {
        let result = analyze("fn s(): string { \"unterminated }");
        assert!(result.module.items.is_empty());
        assert_eq!(result.diagnostics().len(), 1);
    }

    #[test]
    fn diagnostics_are_in_source_order() {
        let result = analyze("fn a(): string { 1 }\nfn b(): string { 2 }");
        let diagnostics = result.diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].span.start < diagnostics[1].span.start);
    }
}
