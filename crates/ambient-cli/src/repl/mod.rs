//! REPL (Read-Eval-Print-Loop) implementation.
//!
//! The REPL is a thin frontend over the same pipeline that powers `ambient
//! check`, the LSP, and `ambient run`. Each session is modeled as an
//! accumulating in-memory module named `repl`: every turn re-checks and
//! re-compiles the committed definitions (plus the new input) through
//! `ambient_analysis`/`ambient_engine`, so the REPL gets the full language —
//! type checking, every item kind, cross-module `use`, real shared
//! diagnostics, and the full platform ability set — without a bespoke
//! parallel compiler. "What is an error" and "how to compile a module set"
//! live in the shared layer, never here (see AGENTS.md).

mod completer;
mod editor;
mod highlighter;

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config as RustylineConfig, Editor, EventHandler, KeyEvent};

use editor::ExternalEditorHandler;

use ambient_analysis::Diagnostic;
use ambient_analysis::package::AnalysisPackage;
use ambient_engine::ability_resolver::DynAbility;
use ambient_engine::ast::{Item, ItemKind};
use ambient_engine::build::{BuildOptions, compile_session_module};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_engine::fqn::NameKey;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleInfo, ModuleRegistry};
use ambient_engine::value::{ModuleExport, ModuleExportKind, ModuleMemberRef, ModuleValue, Value};
use ambient_parser::ReplInput;
use ambient_platform::process::{EventSink, ProcessEvent};

use crate::commands::host::RuntimeHost;
use crate::commands::{core_context, platform_prelude};
use crate::diagnostic::report_build_error;

/// The module path all session definitions accumulate into.
const REPL_MODULE: &str = "repl";

/// Run the interactive REPL.
pub fn cmd_repl(project_dir: Option<&Path>) -> Result<()> {
    eprintln!("Type :help for commands.");

    // Determine project directory (default to current directory).
    let project_dir = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let mut session = ReplSession::new(&project_dir)?;

    // Create the REPL helper (syntax highlighting).
    let completer = completer::ReplHelper::new();

    // Configure rustyline with our helper.
    let config = RustylineConfig::builder()
        .auto_add_history(true)
        .max_history_size(1000)
        .expect("valid history size")
        .build();

    let mut rl: Editor<completer::ReplHelper, DefaultHistory> =
        Editor::with_config(config).context("failed to initialize readline")?;
    rl.set_helper(Some(completer));

    // Bind Ctrl+E and Ctrl+O to open external editor (like bash's edit-and-execute-command).
    rl.bind_sequence(
        KeyEvent::ctrl('E'),
        EventHandler::Conditional(Box::new(ExternalEditorHandler)),
    );
    rl.bind_sequence(
        KeyEvent::ctrl('O'),
        EventHandler::Conditional(Box::new(ExternalEditorHandler)),
    );

    // Load history from file.
    if let Some(history_path) = get_history_path() {
        if let Some(parent) = history_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = rl.load_history(&history_path);
    }

    loop {
        // Flush stdout before reading (in case any output is buffered).
        let _ = io::stdout().flush();

        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let line = line.trim();

                // Skip empty lines.
                if line.is_empty() {
                    continue;
                }

                // Handle REPL commands.
                if line.starts_with(':') {
                    match handle_repl_command(line) {
                        ReplCommand::Help => print_repl_help(),
                        ReplCommand::Quit => break,
                        ReplCommand::Clear => match session.clear() {
                            Ok(()) => eprintln!("State cleared."),
                            Err(e) => eprintln!("\x1b[1;31merror\x1b[0m: {e}"),
                        },
                        ReplCommand::Unknown(cmd) => {
                            eprintln!("Unknown command: {cmd}");
                            eprintln!("Type :help for available commands.");
                        }
                    }
                    continue;
                }

                // Parse and evaluate the input.
                match session.eval(line) {
                    Ok(Some(value)) => {
                        println!("{}", format_value_colored(&value));
                    }
                    Ok(None) => {
                        // Unit result or definition, don't print.
                    }
                    Err(e) => {
                        eprintln!("\x1b[1;31merror\x1b[0m: {e}");
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                eprintln!("^C");
                // Continue the REPL on Ctrl+C.
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                bail!("readline error: {err}");
            }
        }
    }

    // Save history to file.
    if let Some(history_path) = get_history_path() {
        let _ = rl.save_history(&history_path);
    }

    Ok(())
}

/// One committed session input: the raw source the user typed plus the names
/// it binds, used to replace same-named earlier definitions.
struct SessionEntry {
    /// Names this input binds (functions, consts, types, imports). Empty for
    /// `impl` blocks, which have no single name.
    names: Vec<Arc<str>>,
    /// The raw source text of the input, re-emitted verbatim each turn.
    source: String,
}

/// Everything the REPL accumulates and reuses across turns.
struct ReplSession {
    /// Committed definitions, in insertion order.
    entries: Vec<SessionEntry>,
    /// The `repl` module path all definitions live under.
    repl_path: ModulePath,
    /// The analysis package: an opened project, or an empty in-memory
    /// package when the REPL runs outside a project.
    package: AnalysisPackage,
    /// The project root, if any — used to rebuild base state on `:clear`.
    project_root: Option<PathBuf>,
    /// The cached base build (core, plus the project when present) that each
    /// session module merges onto.
    base: CompiledModule,
    /// The canonical [`NameKey`] linking table for `base`, resolving the
    /// session module's cross-module calls.
    imported_hashes: HashMap<NameKey, blake3::Hash>,
    /// Resolved platform prelude abilities for the compiler.
    prelude: Vec<Arc<DynAbility>>,
    /// Monotonic counter naming synthetic expression entry functions.
    entry_counter: u64,
    /// The runtime host that executes entries with the full platform set.
    host: RuntimeHost,
}

impl ReplSession {
    /// Build a fresh session: resolve the project (if any), build the base,
    /// and start the runtime host.
    fn new(project_dir: &Path) -> Result<Self> {
        let repl_path = ModulePath::from_str_segments(&[REPL_MODULE])
            .ok_or_else(|| anyhow!("invalid repl module path"))?;
        let prelude = platform_prelude()?;
        let project_root = find_project_root(project_dir);
        let (package, base, imported_hashes) = build_base(project_root.as_deref(), &prelude)?;
        // The REPL has no program args; `Env::args!()` is empty.
        let host = RuntimeHost::new(noop_event_sink(), Vec::new())?;

        Ok(Self {
            entries: Vec::new(),
            repl_path,
            package,
            project_root,
            base,
            imported_hashes,
            prelude,
            entry_counter: 0,
            host,
        })
    }

    /// Reset all session definitions and rebuild the base + host from scratch.
    fn clear(&mut self) -> Result<()> {
        let (package, base, imported_hashes) =
            build_base(self.project_root.as_deref(), &self.prelude)?;
        self.entries.clear();
        self.package = package;
        self.base = base;
        self.imported_hashes = imported_hashes;
        self.entry_counter = 0;
        self.host = RuntimeHost::new(noop_event_sink(), Vec::new())?;
        // Drop any lingering `repl` module from the package.
        self.sync_repl_module(&self.committed_source());
        Ok(())
    }

    /// The concatenated source of every committed definition.
    fn committed_source(&self) -> String {
        let mut source = String::new();
        for entry in &self.entries {
            source.push_str(&entry.source);
            source.push('\n');
        }
        source
    }

    /// Install `source` as the current `repl` module in the analysis package.
    fn sync_repl_module(&mut self, source: &str) {
        self.package
            .insert_module(self.repl_path.clone(), source.to_string());
    }

    /// Evaluate one line: an introspection query, a definition, or an
    /// expression. Definitions print `Defined: <name>` and return `None`;
    /// expressions return their value (unless `Unit`).
    fn eval(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
        let trimmed = line.trim();

        // Introspection: a `::` path (or bare module name) that names a
        // module or one of its members browses the registry instead of
        // evaluating. Namespaces are addressed with `::`; a `.` is
        // value/field access and never resolves a namespace, so a dotted
        // path falls through to the parser (which rejects it). The
        // path-shape guard keeps ordinary expressions (`1 + 2`, `f(x)`) off
        // the registry-building path.
        if looks_like_path(trimmed)
            && let Some(value) = self.introspect(trimmed)
        {
            return Ok(Some(value));
        }

        // Parse the input as either items or an expression.
        let input = match ambient_parser::parse_repl_input(line) {
            Ok(i) => i,
            Err(e) => return Err(format_repl_parse_error(line, &e)),
        };

        match input {
            ReplInput::Items(items) => self.eval_items(line, &items),
            ReplInput::Expr(_) => self.eval_expression(line),
        }
    }

    /// Type-check and commit a definition input, replacing any same-named
    /// earlier definitions.
    fn eval_items(
        &mut self,
        line: &str,
        items: &[Item],
    ) -> std::result::Result<Option<Value>, String> {
        let committed = self.committed_source();
        let trial_source = format!("{committed}{line}\n");

        // Type-check the whole trial module; commit only if it is clean.
        let registry = self.check_trial(&trial_source)?;

        // Compile to surface any codegen error before mutating state (the
        // merged module is discarded — definitions produce no value).
        self.compile_trial(&registry, &trial_source)
            .map_err(|e| format!("{e}"))?;

        // Commit: drop earlier entries this input redefines, then append.
        let names: Vec<Arc<str>> = items.iter().filter_map(item_name).collect();
        self.commit_entry(SessionEntry {
            names: names.clone(),
            source: line.to_string(),
        });
        self.sync_repl_module(&self.committed_source());

        let defined: Vec<Arc<str>> = items.iter().filter_map(definition_name).collect();
        for name in &defined {
            eprintln!("Defined: {name}");
        }
        Ok(None)
    }

    /// Wrap an expression in a synthetic entry function, run it, and return
    /// its value.
    fn eval_expression(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
        self.entry_counter += 1;
        let entry_local = format!("__repl_entry_{}", self.entry_counter);
        let committed = self.committed_source();
        // The entry is private and unannotated, so effect inference gives it
        // whatever abilities the expression performs — no `with` clause or
        // `use` needed for fully-qualified platform calls.
        let trial_source = format!("{committed}fn {entry_local}() {{\n{line}\n}}\n");

        let registry = self.check_trial(&trial_source)?;
        let merged = self
            .compile_trial(&registry, &trial_source)
            .map_err(|e| format!("{e}"))?;

        // The synthetic entry is never committed; restore the package's
        // `repl` module to the committed state.
        self.sync_repl_module(&committed);

        let entry_qualified = format!("{REPL_MODULE}::{entry_local}");
        let outcome = self
            .host
            .deploy(&merged, &entry_qualified)
            .map_err(|e| format!("{e}"))?;

        if matches!(outcome.value, Value::Unit) {
            Ok(None)
        } else {
            Ok(Some(outcome.value))
        }
    }

    /// Run the shared analysis over a trial `repl` module and reject the turn
    /// if it produces any diagnostics. On success the built registry (with
    /// the trial module resolved) is returned for the caller to compile
    /// against, avoiding a second registry build.
    fn check_trial(&mut self, trial_source: &str) -> std::result::Result<ModuleRegistry, String> {
        self.sync_repl_module(trial_source);
        let registry = self.package.build_registry();
        let result = ambient_analysis::analyze_with_registry(
            trial_source,
            Some(&self.repl_path),
            Some(&registry),
        );
        let diagnostics = result.diagnostics();
        if diagnostics.is_empty() {
            Ok(registry)
        } else {
            // Leave the committed module in place for the next turn.
            let committed = self.committed_source();
            self.sync_repl_module(&committed);
            Err(format_repl_diagnostics(trial_source, &diagnostics))
        }
    }

    /// Compile the (already type-clean) trial `repl` module against `registry`
    /// and merge it onto the base.
    fn compile_trial(
        &self,
        registry: &ModuleRegistry,
        trial_source: &str,
    ) -> Result<CompiledModule> {
        let info = registry
            .get(&self.repl_path)
            .ok_or_else(|| anyhow!("repl module missing from registry"))?;
        let module = info.module.as_ref().clone();
        compile_session_module(
            &self.base,
            registry,
            &module,
            &self.repl_path,
            trial_source,
            self.imported_hashes.clone(),
            &self.prelude,
        )
        .map_err(report_build_error)
    }

    /// Drop committed entries whose names or source this input replaces, then
    /// append it.
    fn commit_entry(&mut self, entry: SessionEntry) {
        self.entries.retain(|existing| {
            existing.source != entry.source
                && !existing.names.iter().any(|n| entry.names.contains(n))
        });
        self.entries.push(entry);
    }

    /// Browse the registry for a module path or member path, if `path` names
    /// one. Bare value names (variables, constants, functions) return `None`
    /// so they evaluate normally.
    fn introspect(&self, path: &str) -> Option<Value> {
        let registry = self.package.build_registry();

        // Whole module: `core`, `core::collections::List`, `repl`.
        if let Some(module_path) = parse_module_path(path)
            && let Some(info) = registry.get(&module_path)
        {
            return Some(Value::Module(Arc::new(module_value(path, info))));
        }

        // Member: `core::collections::List::first`. Split the trailing name
        // off and look it up in the parent module's exports (raw, so the
        // user's own non-`pub` items are browsable too).
        let (parent, member) = path.rsplit_once("::")?;
        let parent_path = parse_module_path(parent)?;
        let info = registry.get(&parent_path)?;
        let export = info.exports.get(member)?;
        Some(Value::ModuleMember(Arc::new(ModuleMemberRef {
            path: path.into(),
            kind: export_kind(export.kind),
        })))
    }
}

/// A no-op process event sink: the REPL is quiet about process lifecycle.
fn noop_event_sink() -> EventSink {
    Arc::new(|_event: &ProcessEvent| {})
}

/// Build the base compiled module and its analysis package.
///
/// With a project, the base is the full package build (core + project, names
/// qualified) and the package is opened from disk. Without one, the base is
/// just the core library and the package is an empty in-memory shell.
fn build_base(
    project_root: Option<&Path>,
    prelude: &[Arc<DynAbility>],
) -> Result<(
    AnalysisPackage,
    CompiledModule,
    HashMap<NameKey, blake3::Hash>,
)> {
    match project_root {
        Some(root) => {
            let result = ambient_engine::build::build_package(
                root,
                crate::commands::parse_source,
                &BuildOptions {
                    platform_source: ambient_platform::ABILITY_DECLARATIONS,
                    prelude_abilities: prelude,
                    progress: None,
                },
            )
            .map_err(report_build_error)?;
            let package = AnalysisPackage::open(root).map_err(|e| anyhow!(e))?;
            Ok((package, result.compiled, result.link_table))
        }
        None => {
            let core = core_context()?;
            let package = AnalysisPackage::empty(PathBuf::from("."), PathBuf::from("."));
            Ok((package, core.compiled, core.hashes))
        }
    }
}

/// Walk up from `dir` looking for an `ambient.toml` that marks a project root.
fn find_project_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir;
    loop {
        if current.join("ambient.toml").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

/// The name a definition input binds, for redefinition bookkeeping. `impl`
/// blocks have no single name and return `None`.
fn item_name(item: &Item) -> Option<Arc<str>> {
    match &item.kind {
        ItemKind::Function(def) => Some(def.name.clone()),
        ItemKind::Const(def) => Some(def.name.clone()),
        ItemKind::Struct(def) => Some(def.name.clone()),
        ItemKind::TypeAlias(def) => Some(def.name.clone()),
        ItemKind::Enum(def) => Some(def.name.clone()),
        ItemKind::Ability(def) => Some(def.name.clone()),
        ItemKind::Trait(def) => Some(def.name.clone()),
        ItemKind::ExternFn(def) => Some(def.name.clone()),
        ItemKind::Use(def) => def
            .alias
            .as_ref()
            .map(|(name, _)| name.clone())
            .or_else(|| def.path.last().map(|(name, _)| name.clone())),
        ItemKind::Impl(_) => None,
    }
}

/// The name to announce as `Defined: <name>`. Imports and impls stay quiet.
fn definition_name(item: &Item) -> Option<Arc<str>> {
    match &item.kind {
        ItemKind::Use(_) | ItemKind::Impl(_) => None,
        _ => item_name(item),
    }
}

/// Map a registry export kind to its value-level rendering kind.
fn export_kind(kind: ExportKind) -> ModuleExportKind {
    match kind {
        ExportKind::Function => ModuleExportKind::Function,
        ExportKind::Const => ModuleExportKind::Const,
        ExportKind::Struct | ExportKind::TypeAlias => ModuleExportKind::Type,
        ExportKind::Enum => ModuleExportKind::Enum,
        ExportKind::EnumVariant => ModuleExportKind::Variant,
        ExportKind::Ability | ExportKind::Trait => ModuleExportKind::Ability,
    }
}

/// Build a browsable module value from a registry module's exports and
/// re-exported children.
fn module_value(path: &str, info: &ModuleInfo) -> ModuleValue {
    let mut exports: Vec<ModuleExport> = info
        .exports
        .values()
        .map(|e| ModuleExport::new(e.name.as_ref(), export_kind(e.kind)))
        .collect();
    for re in &info.re_exports {
        if let Some(name) = re.exported_name() {
            exports.push(ModuleExport::new(name, ModuleExportKind::Module));
        }
    }
    ModuleValue::new(path, exports)
}

/// Whether `s` is shaped like a module/member path: one or more identifier
/// segments joined by `::`, nothing else. Ordinary expressions (with
/// operators, whitespace, calls, or a `.`) are not path-shaped and skip
/// introspection so they never trigger a registry build.
fn looks_like_path(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|seg| {
            let mut chars = seg.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Parse a `::`-separated module path, rejecting empty input.
fn parse_module_path(path: &str) -> Option<ModulePath> {
    if path.is_empty() {
        return None;
    }
    ModulePath::from_segments(path.split("::").map(Arc::from).collect())
}

/// REPL command types.
enum ReplCommand {
    Help,
    Quit,
    Clear,
    Unknown(String),
}

/// Parse a REPL command.
fn handle_repl_command(line: &str) -> ReplCommand {
    let cmd = line.trim_start_matches(':').split_whitespace().next();
    match cmd {
        Some("help") | Some("h") | Some("?") => ReplCommand::Help,
        Some("quit") | Some("q") | Some("exit") => ReplCommand::Quit,
        Some("clear") | Some("reset") => ReplCommand::Clear,
        Some(other) => ReplCommand::Unknown(other.to_string()),
        None => ReplCommand::Unknown(String::new()),
    }
}

/// Print REPL help.
fn print_repl_help() {
    eprintln!("REPL Commands:");
    eprintln!("  :help, :h, :?    Show this help message");
    eprintln!("  :quit, :q, :exit Exit the REPL");
    eprintln!("  :clear, :reset   Clear all defined functions and variables");
    eprintln!();
    eprintln!("Key Bindings:");
    eprintln!("  Ctrl+E, Ctrl+O   Edit current line in $EDITOR");
    eprintln!();
    eprintln!("Definitions:");
    eprintln!("  fn add(x, y) {{ x + y }}   Define a function");
    eprintln!("  const PI: Number = 3      Define a constant (must be a literal)");
    eprintln!("  struct Point {{ x: Number, y: Number }}   Define a type");
    eprintln!("  use core::system::Stdio   Import across turns");
    eprintln!();
    eprintln!("Expressions:");
    eprintln!("  1 + 2 * 3        Evaluate an expression");
    eprintln!("  add(1, 2)        Call a defined function");
    eprintln!("  \"hello\"          String literal");
    eprintln!();
    eprintln!("The REPL runs the full pipeline: type checking, every item kind");
    eprintln!("(struct/enum/type/ability/trait/impl/use), cross-module `use`, and");
    eprintln!("the same diagnostics as `ambient check`. A `const` must be a literal;");
    eprintln!("use a `fn` for a computed value.");
}

/// Format a parse error for REPL display (without file path).
fn format_repl_parse_error(line: &str, error: &ambient_parser::ParseError) -> String {
    let start = error.span.start as usize;
    let end = error.span.end as usize;

    // For single-line REPL input, we can show a caret pointing to the error.
    let col = start.min(line.len());
    let underline_len = (end - start).min(line.len().saturating_sub(col)).max(1);

    let spaces = " ".repeat(col);
    let carets = "^".repeat(underline_len);

    let mut msg = format!("{}\n", error.kind);
    msg.push_str(&format!("  {line}\n"));
    msg.push_str(&format!("  {spaces}{carets}"));

    if let Some(ctx) = &error.context {
        msg.push_str(&format!("\n  note: {ctx}"));
    }

    msg
}

/// Render shared analysis diagnostics against the trial module source.
///
/// The trial source is `committed definitions + this turn's input`, so a
/// caret can point at the exact offending line even when the error is in the
/// synthetic entry wrapper.
fn format_repl_diagnostics(source: &str, diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    for (i, diag) in diagnostics.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (col, line_start, line_end) = line_info(source, diag.span.start as usize);
        let line_content = &source[line_start..line_end];

        out.push_str(&diag.message);
        out.push('\n');
        out.push_str(&format!("  {line_content}\n"));

        let underline_start = col;
        let underline_len = ((diag.span.end - diag.span.start) as usize)
            .min(line_content.len().saturating_sub(underline_start))
            .max(1);
        out.push_str(&format!(
            "  {}{}",
            " ".repeat(underline_start),
            "^".repeat(underline_len)
        ));

        if let Some(note) = &diag.note {
            out.push_str(&format!("\n  note: {note}"));
        }
    }
    out
}

/// Column offset within its line and the line's byte bounds for `offset`.
fn line_info(source: &str, offset: usize) -> (usize, usize, usize) {
    let mut line_start = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line_start = i + 1;
        }
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);
    (offset.saturating_sub(line_start), line_start, line_end)
}

/// Get the history file path.
fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("ambient").join("repl_history"))
}
