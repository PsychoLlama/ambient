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
    clippy::self_named_module_files
)]
#![cfg_attr(not(test), deny(clippy::expect_used, clippy::unwrap_used))]
#![allow(clippy::missing_errors_doc)]

pub mod completions;
pub mod core_cache;
pub mod occurrences;
pub mod package;
pub mod queries;
pub mod session;
pub mod symbols;

use ambient_engine::ability_resolver::AbilityResolver;
use ambient_engine::ast::{Module, Span};
use ambient_engine::infer::{
    BoxedTypeError, CheckResult, check_module_with_registry,
    check_module_with_registry_and_resolver,
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

impl Diagnostic {
    /// An error-severity diagnostic from raw parts. The single place a
    /// `Diagnostic` is constructed, so every frontend renders the same shape.
    #[must_use]
    pub fn error(span: Span, message: String, note: Option<String>) -> Self {
        Self {
            span,
            message,
            note,
            severity: Severity::Error,
        }
    }

    /// Convert a parse error to its rendered diagnostic.
    #[must_use]
    pub fn from_parse_error(error: &ParseError) -> Self {
        Self::error(error.span, error.kind.to_string(), error.context.clone())
    }

    /// Convert a type error to its rendered diagnostic.
    #[must_use]
    pub fn from_type_error(error: &BoxedTypeError) -> Self {
        Self::error(
            Span::new(error.span.0, error.span.1),
            error.kind.to_string(),
            error.context.clone(),
        )
    }
}

/// Render a set of type errors as diagnostics, in `ambient check` order.
///
/// The build pipeline (`ambient run`/`build`/`dev`) type-checks with the
/// same engine entry point as analysis but reports through the engine's
/// `BuildError`. This is the one conversion both it and
/// [`AnalysisResult::diagnostics`] use, so a type error renders identically
/// no matter which command surfaced it — the parity invariant, extended to
/// the compiling commands.
#[must_use]
pub fn type_error_diagnostics(errors: &[BoxedTypeError]) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = errors.iter().map(Diagnostic::from_type_error).collect();
    out.sort_by_key(|d| (d.span.start, d.span.end));
    out
}

/// The result of analyzing one module's source.
///
/// `Clone` so the incremental analysis [`session`] can memoize a module's
/// check-level result (parse errors, type errors, typed AST) and replay it
/// on a warm hit — byte-identical to a cold analysis. The `import_cycle`
/// field is *not* part of the memoized value: a cycle is a package-graph
/// fact the session recomputes per registry revision and overlays, because a
/// dependent's edit can create or dissolve a cycle without moving this
/// module's own cache-key inputs.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// All parse and lowering errors, in source order.
    pub parse_errors: Vec<ParseError>,
    /// Type errors from checking the (possibly partial) module.
    pub type_errors: Vec<BoxedTypeError>,
    /// The typed AST. Always present — files that fail to lex entirely
    /// yield an empty module. Items that failed to parse are missing.
    pub module: Module,
    /// An import-cycle diagnostic when this module participates in a
    /// module dependency cycle (the engine's [`ambient_engine::module_cycles`]
    /// decision). Reported independently of parse/type errors — the cycle is
    /// a package-structural fact, so it is not subject to the "suppress type
    /// errors while parse errors exist" policy.
    pub import_cycle: Option<Diagnostic>,
}

impl AnalysisResult {
    /// Whether any reportable problem was found.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.parse_errors.is_empty() || !self.type_errors.is_empty() || self.import_cycle.is_some()
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
        diagnostics_from(
            &self.parse_errors,
            &self.type_errors,
            self.import_cycle.as_ref(),
        )
    }
}

/// The diagnostics-reporting policy, shared by [`AnalysisResult`] and the
/// REPL's [`SessionCheck`] so both frontends decide "what is an error"
/// identically (the AGENTS invariant: the decision lives in `ambient-analysis`,
/// never a frontend). While parse errors exist, type errors are suppressed;
/// an import cycle is always reported.
#[must_use]
pub fn diagnostics_from(
    parse_errors: &[ParseError],
    type_errors: &[BoxedTypeError],
    import_cycle: Option<&Diagnostic>,
) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = parse_errors
        .iter()
        .map(Diagnostic::from_parse_error)
        .collect();

    if out.is_empty() {
        out.extend(type_errors.iter().map(Diagnostic::from_type_error));
    }

    // An import cycle is a structural fact independent of parse/type errors, so
    // it is always reported (its span-0 anchor sorts it first).
    if let Some(cycle) = import_cycle {
        out.push(cycle.clone());
    }

    out.sort_by_key(|d| (d.span.start, d.span.end));
    out
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
///
/// Built once per thread and cloned out (module ASTs are `Arc`-shared, so
/// the clone is cheap). This sits on hot paths — the REPL re-analyzes on
/// every keystroke — so re-parsing the core library each call is not an
/// option.
#[must_use]
pub fn core_platform_registry() -> ModuleRegistry {
    thread_local! {
        static CACHE: std::cell::OnceCell<ModuleRegistry> = const { std::cell::OnceCell::new() };
    }
    CACHE.with(|cache| cache.get_or_init(build_core_platform_registry).clone())
}

fn build_core_platform_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();

    let parse = |source: &str| ambient_parser::parse(source).map_err(|e| e.to_string());
    let core_paths = ambient_engine::core_library::register_core_modules(&mut registry, parse)
        .unwrap_or_default();
    let platform_paths = ambient_engine::core_library::register_declaration_modules(
        &mut registry,
        ambient_platform::platform_modules(),
        parse,
    )
    .unwrap_or_default();

    // Re-register the builtins resolved — the same canonicalization
    // `build_package` performs — so both frontends derive byte-identical
    // builtin interfaces and hydrate foreign builtin signatures in canonical
    // (`Fqn`) form. Built once per thread and cloned out, so this runs once.
    let builtin_paths: Vec<_> = core_paths.into_iter().chain(platform_paths).collect();
    ambient_engine::core_library::resolve_builtin_modules(&mut registry, &builtin_paths);

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
    let mut result = check_without_cycle(source, module_path, registry, resolver);

    // Import cycles are a package-level property, so they only apply when a
    // real package registry + module path were supplied (the single-file
    // fallback has no cross-module edges). The decision lives in the engine
    // (`module_cycles`), shared with `build_package`, so `ambient check` and
    // the LSP — both of which route through this function — report the same
    // rendering the compiler does, at every module participating in the cycle.
    // The incremental [`session`] bypasses this per-module derivation and
    // overlays a batch-computed cycle set instead (same rendering).
    result.import_cycle = match (module_path, registry) {
        (Some(path), Some(reg)) => {
            ambient_engine::module_cycles::import_cycle_containing(reg, path)
                .map(|cycle| Diagnostic::error(Span::new(0, 0), cycle.describe(), None))
        }
        _ => None,
    };
    result
}

/// A REPL turn's single type-check: the diagnostics `analyze_with_registry`
/// would report *plus* the engine [`CheckResult`] the compiler needs — so a
/// turn type-checks its accumulated module exactly once instead of twice
/// (once to gate the turn on diagnostics, once again inside compilation).
///
/// The typed module and its canonical signatures travel in [`Self::check`];
/// [`compile_session_module`](ambient_engine::build::compile_session_module)
/// consumes them directly rather than re-running inference.
pub struct SessionCheck {
    /// Parse/lowering errors, in source order (from `parse_recovering`).
    pub parse_errors: Vec<ParseError>,
    /// The engine's check result: type errors, the typed AST, and the
    /// canonical signature of every named item.
    pub check: CheckResult,
    /// An import-cycle diagnostic when this module is in a dependency cycle.
    pub import_cycle: Option<Diagnostic>,
    /// The session entry's resolved return type — the type of the turn's
    /// expression with the final substitution applied. `None` when the
    /// entry didn't parse or its scheme is not a function.
    pub entry_type: Option<ambient_engine::types::Type>,
}

impl SessionCheck {
    /// The diagnostics for this turn, using the shared reporting policy — so a
    /// REPL turn is gated on byte-identically the diagnostics
    /// [`analyze_with_registry`] would produce.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        diagnostics_from(
            &self.parse_errors,
            &self.check.errors,
            self.import_cycle.as_ref(),
        )
    }
}

/// Check a REPL session module against `registry` once, returning both its
/// diagnostics and the engine [`CheckResult`] for compilation.
///
/// This mirrors [`analyze_with_registry`] (same parse, same check entry, same
/// import-cycle overlay) but surfaces the `CheckResult` instead of discarding
/// it, so the REPL avoids a redundant second inference pass per turn. The
/// module must already be registered under `path` alongside the rest of the
/// package and the core/platform declaration modules.
///
/// `entry` names the turn's synthetic entry function and carries the
/// concrete types of the session bindings the host passes as its arguments;
/// the entry's parameter types are pinned to them before bodies check, and
/// its resolved return type comes back as [`SessionCheck::entry_type`].
#[must_use]
pub fn check_session_module(
    source: &str,
    path: &ModulePath,
    registry: &ModuleRegistry,
    entry: &ambient_engine::infer::SessionEntrySpec<'_>,
) -> SessionCheck {
    let recovered = parse_recovering(source);
    let parse_errors = recovered.errors;
    let (check, entry_type) = ambient_engine::infer::check_session_module_with_registry(
        recovered.module,
        path,
        registry,
        entry,
    );
    let import_cycle = ambient_engine::module_cycles::import_cycle_containing(registry, path)
        .map(|cycle| Diagnostic::error(Span::new(0, 0), cycle.describe(), None));
    SessionCheck {
        parse_errors,
        check,
        import_cycle,
        entry_type,
    }
}

/// The check-level analysis of a module: parse errors, type errors, and the
/// typed AST — everything [`analyze_with_registry_and_resolver`] computes
/// **except** the import-cycle overlay. This is the memoizable core: its
/// output is a pure function of the module's own resolved source and the
/// registry's view of its dependencies, exactly what the [`session`] cache
/// key captures. `import_cycle` is always `None`; callers attach it.
#[must_use]
pub fn check_without_cycle(
    source: &str,
    module_path: Option<&ModulePath>,
    registry: Option<&ModuleRegistry>,
    resolver: Option<AbilityResolver>,
) -> AnalysisResult {
    let recovered = parse_recovering(source);
    let mut parse_errors = recovered.errors;
    let module = recovered.module;

    // A user module may not take a reserved root name: its canonical
    // names would shadow the real `core`/`platform` namespaces. The build
    // rejects such packages outright; analysis reports the same fact as a
    // module diagnostic.
    if let Some(path) = module_path
        && path.collides_with_reserved_root()
    {
        parse_errors.push(ambient_parser::ParseError::new(
            ambient_parser::ParseErrorKind::LoweringError(format!(
                "module `{path}` collides with the reserved `{}` namespace; rename the file",
                path.segments()[0]
            )),
            ambient_engine::ast::Span::new(0, 0),
        ));
    }

    // Type check the (possibly partial) module. The registry carries the
    // `platform` declaration module, so `core::system::…` abilities resolve
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
        import_cycle: None,
    }
}

/// Type-check a *healed* completion source: the live document text with the
/// in-progress edit patched just enough to parse (a partial member blanked, a
/// dangling `.` given a placeholder ident).
///
/// Completion often fires exactly when the document doesn't parse — and
/// recovery is item-level, so the item being edited (and with it the cursor's
/// scope) vanishes from the live typed AST. This runs the same pipeline as
/// [`check_without_cycle`] on the healed text, against a copy of `registry`
/// with the healed module registered in place of the broken one, and returns
/// the typed module only when the heal actually parsed. Diagnostics are
/// deliberately discarded: healing exists for position queries, never for
/// reporting.
#[must_use]
pub fn healed_module_for_completion(
    source: &str,
    module_path: &ModulePath,
    registry: &ModuleRegistry,
) -> Option<Module> {
    let recovered = parse_recovering(source);
    if !recovered.errors.is_empty() {
        return None;
    }
    let mut registry = registry.clone();
    registry.register(module_path, std::sync::Arc::new(recovered.module.clone()));
    Some(check_module_with_registry(recovered.module, module_path, &registry).module)
}

/// An ability resolver with the core abilities (`core::exception`) and the
/// platform bindings interface (`core::system`) registered as namespaced
/// dynamics, mirroring how the CLI checks code.
#[must_use]
pub fn platform_prelude_resolver() -> AbilityResolver {
    let mut resolver = AbilityResolver::new();
    // A parse-only core registry supplies the prelude so ability resolution
    // seeds the primitive nominals it hashes against (see the CLI's
    // `platform_prelude`). Without it, ability ids would drift from the CLI's.
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    let registered = ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .is_ok();
    if !registered {
        return resolver;
    }
    // Core's own `ability` declarations (`Exception`) — registered under
    // their declaring module, exactly as a full check seeds them.
    for (fqn, ability) in ambient_engine::infer::resolve_registry_abilities(&registry) {
        resolver.register_dynamic_in_namespace(&fqn.module, (*ability).clone());
    }
    // The platform bindings, under the `core::system` namespace. Each
    // ability lives in its own submodule now (`core::system::stdio`, ...),
    // but they are re-exported by `core::system`'s `main.ab` and spelled
    // `core::system::Stdio` in user code, so the dynamic resolver keeps
    // registering them under the `core::system` namespace.
    for module in ambient_platform::platform_modules() {
        if let Ok(mut parsed) = ambient_parser::parse(module.source) {
            let (abilities, _errors) =
                ambient_engine::infer::resolve_ability_declarations(&mut parsed, &registry);
            for ability in abilities {
                resolver.register_dynamic_in_namespace(
                    &ambient_engine::fqn::ModuleId::core_system(),
                    (*ability).clone(),
                );
            }
        }
    }
    resolver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_source_has_no_diagnostics() {
        let result = analyze("fn add(x: Number, y: Number): Number { x + y }");
        assert!(!result.has_errors());
        assert_eq!(result.module.items.len(), 1);
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn type_error_is_reported() {
        let result = analyze("fn bad(): String { 42 }");
        assert!(result.has_errors());
        assert_eq!(result.parse_errors.len(), 0);
        assert!(!result.type_errors.is_empty());
        assert!(!result.diagnostics().is_empty());
    }

    #[test]
    fn parse_error_still_yields_partial_module() {
        let result = analyze("fn ok(): Number { 1 }\nfn broken(\n");
        assert!(!result.parse_errors.is_empty());
        assert_eq!(result.module.items.len(), 1);
    }

    #[test]
    fn type_errors_suppressed_while_parse_errors_exist() {
        // `bad` has a type error, `broken` a parse error. Reporting shows
        // only the parse error (CLI parity); the type error is still
        // computed for IDE consumers.
        let result = analyze("fn bad(): String { 42 }\nfn broken(\n");
        let diagnostics = result.diagnostics();
        assert!(!result.parse_errors.is_empty());
        assert!(!result.type_errors.is_empty());
        assert_eq!(diagnostics.len(), result.parse_errors.len());
    }

    #[test]
    fn lexer_error_yields_empty_module() {
        let result = analyze("fn s(): String { \"unterminated }");
        assert!(result.module.items.is_empty());
        assert_eq!(result.diagnostics().len(), 1);
    }

    #[test]
    fn diagnostics_are_in_source_order() {
        let result = analyze("fn a(): String { 1 }\nfn b(): String { 2 }");
        let diagnostics = result.diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].span.start < diagnostics[1].span.start);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Rigid type parameters (`Type::Param`)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn generic_identity_function_is_clean() {
        // `T` is rigid in the body: `x: T` and the return `T` are the same
        // `Type::Param`, so they unify and nothing is flagged as undefined.
        let result = analyze("fn id<T>(x: T): T { x }");
        assert!(!result.has_errors(), "{:?}", result.diagnostics());
    }

    #[test]
    fn nested_lambda_reuses_type_param() {
        // A lambda nested in the body inherits the rigid scope, so its `y: T`
        // annotation is the same rigid parameter the argument carries.
        let result = analyze("fn f<T>(x: T): T { let g = (y: T) => y; g(x) }");
        assert!(!result.has_errors(), "{:?}", result.diagnostics());
    }

    #[test]
    fn type_param_return_mismatch_names_the_param() {
        // Returning a `Number` where `T` is declared is a real mismatch, and
        // the diagnostic still names `T` — proving the rigid parameter's
        // source name survives the `Named`→`Param` representation change.
        let result = analyze("fn bad<T>(x: T): T { 1 }");
        assert!(result.has_errors());
        let messages: String = result
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(
            messages.contains('T'),
            "expected the message to name `T`: {messages}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Resolve-or-error for type annotations (Phase 2)
    // ─────────────────────────────────────────────────────────────────────

    /// UUID prefix for the declared enums/structs these tests need.
    const U: &str = "A1B2C3D4-0000-0000-0000-0000000000";

    #[test]
    fn undefined_type_annotation_is_reported_once_without_cascade() {
        // A bare typo in a parameter type is a first-class diagnostic: exactly
        // one `undefined type`, and no secondary "type mismatch" cascade (the
        // annotation is rewritten to `Type::Error`, which unifies away).
        let result = analyze("fn f(x: Strng) { x }");
        let diagnostics = result.diagnostics();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("undefined type"));
        assert!(diagnostics[0].message.contains("Strng"));
    }

    #[test]
    fn undefined_return_type_reports_without_mismatch() {
        // A typo in return position is reported once; the body value does not
        // also trip a return-type mismatch.
        let result = analyze("fn f(): Strng { 1 }");
        let diagnostics = result.diagnostics();
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("Strng"));
        assert!(!diagnostics[0].message.contains("mismatch"));
    }

    #[test]
    fn primitive_and_local_type_annotations_are_clean() {
        // A primitive works registry-backed (guards the registry-primitives
        // path); a locally declared struct used as an annotation is not a
        // false positive.
        assert!(!analyze("fn f(x: String): String { x }").has_errors());
        let src =
            format!("unique({U}01) struct Point {{ x: Number }}\nfn f(p: Point): Number {{ p.x }}");
        let result = analyze(&src);
        assert!(!result.has_errors(), "{:?}", result.diagnostics());
    }

    #[test]
    fn known_generic_type_is_clean_but_unknown_head_errors() {
        // A known generic head (`Option`) with a valid argument is clean;
        // checking the *head* name still catches an unknown constructor.
        assert!(!analyze("fn f(x: Option<Number>): Option<Number> { x }").has_errors());
        let result = analyze("fn f(x: Nope<Number>) { x }");
        let messages: String = result
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(messages.contains("Nope"), "{messages}");
    }

    #[test]
    fn recursive_enum_annotation_is_clean() {
        // A self-referential enum resolves: the payload `IntList` already
        // carries its uuid by the time the declared-types sweep runs.
        let src = format!(
            "unique({U}02) enum IntList {{ Cons((Number, IntList)), Nil }}\n\
             fn f(l: IntList): IntList {{ l }}"
        );
        let result = analyze(&src);
        assert!(!result.has_errors(), "{:?}", result.diagnostics());
    }

    #[test]
    fn undefined_type_in_struct_field_is_reported() {
        let src = format!("unique({U}03) struct S {{ x: Strng }}");
        let result = analyze(&src);
        let messages: String = result
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(messages.contains("undefined type"), "{messages}");
        assert!(messages.contains("Strng"), "{messages}");
    }

    #[test]
    fn generic_enum_type_param_payload_is_not_flagged() {
        // A generic enum's own parameter used as a payload must not be
        // mistaken for an undefined type.
        let src = format!(
            "unique({U}04) enum Box<T> {{ Wrap(T), Empty }}\n\
             fn f(b: Box<Number>): Box<Number> {{ b }}"
        );
        let result = analyze(&src);
        assert!(!result.has_errors(), "{:?}", result.diagnostics());
    }

    #[test]
    fn undefined_type_in_let_annotation_is_reported() {
        let result = analyze("fn f() { let x: Strng = 1; x }");
        let messages: String = result
            .diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(messages.contains("Strng"), "{messages}");
    }
}
