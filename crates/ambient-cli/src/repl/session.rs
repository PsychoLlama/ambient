//! The REPL session: the accumulating in-memory module plus the runtime
//! host every turn deploys onto, and the REPL command handling.
//!
//! This is the engine behind both frontends: the interactive [`cmd_repl`]
//! rustyline loop (`super`) and the in-process integration-test harness
//! (`crates/ambient-cli/tests/repl_harness`). It owns "what a turn does"
//! and routes every out-of-band line — `Defined:` announcements, deploy
//! notes/warnings, task lifecycle events, `State cleared.` — through an
//! injectable [`ReplIo`], plus a [`StdioSink`] for program (`Stdio`/`Log`)
//! output. The terminal frontend sends control lines to stderr and inherits
//! real stdout/stderr; a test routes both into one buffer so everything a
//! turn emits can be asserted on without a PTY.
//!
//! See the module-level docs on [`super`] for the deploy-frontend design.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};

use ambient_analysis::Diagnostic;
use ambient_analysis::package::AnalysisPackage;
use ambient_engine::ast::{Item, ItemKind};
use ambient_engine::build::{BuildOptions, compile_session_module};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::fqn::NameKey;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleInfo, ModuleRegistry};
use ambient_engine::value::{ModuleExport, ModuleExportKind, ModuleMemberRef, ModuleValue, Value};
use ambient_parser::ReplInput;
use ambient_platform::{StdioSink, TaskEvent, TaskEventSink};

use crate::commands::core_context;
use crate::commands::host::{HostDeployOutcome, RuntimeHost};
use crate::diagnostic::report_build_error;

/// The module path all session definitions accumulate into.
const REPL_MODULE: &str = "repl";

/// The message emitted to the control sink after a successful `:clear`.
pub const STATE_CLEARED: &str = "State cleared.";

/// A control-line sink: the target for everything the session would print
/// to a terminal's stderr — `Defined:` lines, deploy notes/warnings, task
/// lifecycle events, and `State cleared.`.
pub type ControlSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Where a session's out-of-band output goes.
///
/// Two channels: control lines (via [`ControlSink`]) and program
/// `Stdio`/`Log` output (via [`StdioSink`], handed to the [`RuntimeHost`]).
/// The terminal frontend sends control lines to stderr and inherits the
/// real stdout/stderr for the program; a test routes both into one buffer.
pub struct ReplIo {
    control: ControlSink,
    program: StdioSink,
}

impl ReplIo {
    /// Build an IO configuration from explicit control and program sinks.
    #[must_use]
    pub fn new(control: ControlSink, program: StdioSink) -> Self {
        Self { control, program }
    }

    /// The interactive frontend's IO: control lines to stderr, program
    /// output inherited from the real stdout/stderr.
    #[must_use]
    pub fn terminal() -> Self {
        Self {
            control: Arc::new(|line: &str| eprintln!("{line}")),
            program: StdioSink::inherit(),
        }
    }
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
pub struct ReplSession {
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
    /// Monotonic counter naming synthetic expression entry functions.
    entry_counter: u64,
    /// The runtime host that executes entries with the full platform set.
    host: RuntimeHost,
    /// Where control lines and program output go.
    io: ReplIo,
}

impl ReplSession {
    /// Build a fresh session: resolve the project (if any), build the base,
    /// and start the runtime host wired to `io`.
    pub fn new(project_dir: &Path, io: ReplIo) -> Result<Self> {
        let repl_path = ModulePath::from_str_segments(&[REPL_MODULE])
            .ok_or_else(|| anyhow!("invalid repl module path"))?;
        let project_root = find_project_root(project_dir);
        let (package, base, imported_hashes) = build_base(project_root.as_deref())?;
        // The REPL has no program args; `Env::args!()` is empty.
        let host = RuntimeHost::new(
            task_event_sink(Arc::clone(&io.control)),
            io.program.clone(),
            Vec::new(),
        )?;

        Ok(Self {
            entries: Vec::new(),
            repl_path,
            package,
            project_root,
            base,
            imported_hashes,
            entry_counter: 0,
            host,
            io,
        })
    }

    /// Emit a control line (a `Defined:`, a deploy note, a task event, …).
    fn control(&self, line: &str) {
        (self.io.control)(line);
    }

    /// Wind the running program down: drain every task and wait for it to
    /// stop, so a lingering ticker can't print into a dropped buffer or
    /// keep the test process alive.
    pub fn shutdown(&self) {
        self.host.tasks().drain_all();
        self.host.tasks().wait_all();
    }

    /// Reset all session definitions and rebuild the base + host from
    /// scratch, then announce `State cleared.` on the control sink.
    ///
    /// The running program winds down first: tasks are drained and waited
    /// for (bounded by the drain deadline), so a lingering ticker can't
    /// keep printing into the fresh session. Errors are returned for the
    /// frontend to render.
    pub fn clear(&mut self) -> Result<()> {
        self.shutdown();
        let (package, base, imported_hashes) = build_base(self.project_root.as_deref())?;
        self.entries.clear();
        self.package = package;
        self.base = base;
        self.imported_hashes = imported_hashes;
        self.entry_counter = 0;
        self.host = RuntimeHost::new(
            task_event_sink(Arc::clone(&self.io.control)),
            self.io.program.clone(),
            Vec::new(),
        )?;
        // Drop any lingering `repl` module from the package.
        self.sync_repl_module(&self.committed_source());
        self.control(STATE_CLEARED);
        Ok(())
    }

    /// Execute a parsed REPL command. [`ReplCommand::Quit`] is a no-op here
    /// (the frontend decides how to exit); the rest write through the
    /// control sink. A failed `:clear` is returned for the frontend to
    /// render.
    pub fn run_command(&mut self, command: ReplCommand) -> Result<()> {
        match command {
            ReplCommand::Help => {
                let control = Arc::clone(&self.io.control);
                write_repl_help(&|line| control(line));
                Ok(())
            }
            ReplCommand::Clear => self.clear(),
            ReplCommand::Quit => Ok(()),
            ReplCommand::Unknown(cmd) => {
                self.control(&format!("Unknown command: {cmd}"));
                self.control("Type :help for available commands.");
                Ok(())
            }
        }
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
    /// expression. Definitions announce `Defined: <name>` and return `None`;
    /// expressions return their value (unless `Unit`).
    pub fn eval(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
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

    /// Type-check, deploy, and commit a definition input, replacing any
    /// same-named earlier definitions.
    ///
    /// A definition turn is a deploy: the trial build ships as a
    /// generation whose reconciliation entry is a synthetic no-op — the
    /// point is the validate-and-swap, which rebinds the redefined name
    /// for every late-bound point of the running program (a task's next
    /// pass, a `Live::latest!` read). A rejected deploy (a failed
    /// migration check) errors the turn: nothing is committed, the name
    /// table was never swapped, and the running program is untouched.
    fn eval_items(
        &mut self,
        line: &str,
        items: &[Item],
    ) -> std::result::Result<Option<Value>, String> {
        self.entry_counter += 1;
        let entry_local = format!("__repl_entry_{}", self.entry_counter);
        let committed = self.committed_source();
        let trial_source = format!("{committed}{line}\nfn {entry_local}() {{ }}\n");

        // Type-check the whole trial module once; commit only if it is clean.
        let (registry, check) = self.check_trial(&trial_source)?;

        let merged = self
            .compile_trial(&registry, &check, &trial_source)
            .map_err(|e| format!("{e}"))?;

        let entry_qualified = format!("{REPL_MODULE}::{entry_local}");
        let outcome = match self.host.deploy_incremental(&merged, &entry_qualified) {
            Ok(outcome) => outcome,
            Err(e) => {
                // Leave the committed module in place for the next turn.
                self.sync_repl_module(&committed);
                return Err(format!("{e}"));
            }
        };

        // Commit: drop earlier entries this input redefines, then append.
        let names: Vec<Arc<str>> = items.iter().filter_map(item_name).collect();
        self.commit_entry(SessionEntry {
            names,
            source: line.to_string(),
        });
        self.sync_repl_module(&self.committed_source());

        let defined: Vec<Arc<str>> = items.iter().filter_map(definition_name).collect();
        for name in &defined {
            self.control(&format!("Defined: {name}"));
        }
        self.report_deploy(&outcome);
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

        let (registry, check) = self.check_trial(&trial_source)?;
        let merged = self
            .compile_trial(&registry, &check, &trial_source)
            .map_err(|e| format!("{e}"))?;

        // The synthetic entry is never committed; restore the package's
        // `repl` module to the committed state.
        self.sync_repl_module(&committed);

        let entry_qualified = format!("{REPL_MODULE}::{entry_local}");
        let outcome = self
            .host
            .deploy_incremental(&merged, &entry_qualified)
            .map_err(|e| format!("{e}"))?;
        self.report_deploy(&outcome);

        if matches!(outcome.report.value, Value::Unit) {
            Ok(None)
        } else {
            Ok(Some(outcome.report.value))
        }
    }

    /// Type-check a trial `repl` module once and reject the turn if it
    /// produces any diagnostics. On success the built registry *and* the
    /// single [`SessionCheck`](ambient_analysis::SessionCheck) (its typed AST
    /// and canonical signatures) are returned, so the caller compiles from the
    /// same check instead of re-running inference — a turn type-checks exactly
    /// once.
    fn check_trial(
        &mut self,
        trial_source: &str,
    ) -> std::result::Result<(ModuleRegistry, ambient_analysis::SessionCheck), String> {
        self.sync_repl_module(trial_source);
        let registry = self.package.build_registry();
        let check =
            ambient_analysis::check_session_module(trial_source, &self.repl_path, &registry);
        let diagnostics = check.diagnostics();
        if diagnostics.is_empty() {
            Ok((registry, check))
        } else {
            // Leave the committed module in place for the next turn.
            let committed = self.committed_source();
            self.sync_repl_module(&committed);
            Err(format_repl_diagnostics(trial_source, &diagnostics))
        }
    }

    /// Compile the trial `repl` module — reusing the turn's single
    /// [`SessionCheck`](ambient_analysis::SessionCheck) rather than
    /// re-inferring — and merge it onto the base.
    fn compile_trial(
        &self,
        registry: &ModuleRegistry,
        check: &ambient_analysis::SessionCheck,
        trial_source: &str,
    ) -> Result<CompiledModule> {
        compile_session_module(
            &self.base,
            registry,
            &check.check,
            &self.repl_path,
            trial_source,
            self.imported_hashes.clone(),
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

        // Whole module: `core`, `core::collections::list`, `repl`.
        if let Some(module_path) = parse_module_path(path)
            && let Some(info) = registry.get(&module_path)
        {
            return Some(Value::Module(Arc::new(module_value(path, info))));
        }

        // Member: `core::option::flatten`. Split the trailing name
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

    /// Narrate what a turn's deploy did to the running program: names the
    /// rebinding rule retired (signature changed — running references keep
    /// resolving to the old code) and the deploy warnings.
    fn report_deploy(&self, outcome: &HostDeployOutcome) {
        for name in &outcome.report.names.retired {
            self.control(&format!(
                "note: `{name}` changed signature — retired, not rebound; \
                 running references keep the old code"
            ));
        }
        for warning in &outcome.report.warnings {
            self.control(&format!("\x1b[1;33mwarning\x1b[0m: {warning}"));
        }
    }
}

/// Narrate task lifecycle to `control`: tasks print from their own threads,
/// so this is how an ensure or drain from three turns ago stays legible.
fn task_event_sink(control: ControlSink) -> TaskEventSink {
    Arc::new(move |event: &TaskEvent| match event {
        TaskEvent::Started { name } => control(&format!("task `{name}` started")),
        TaskEvent::Draining { name } => control(&format!("task `{name}` draining")),
        TaskEvent::Drained { name, .. } => control(&format!("task `{name}` drained")),
        TaskEvent::Faulted {
            name,
            error,
            restarting,
        } => {
            control(&format!("task `{name}` fault: {error}"));
            if !restarting {
                control(&format!("task `{name}` parked"));
            }
        }
    })
}

/// Build the base compiled module and its analysis package.
///
/// With a project, the base is the full package build (core + project, names
/// qualified) and the package is opened from disk. Without one, the base is
/// just the core library and the package is an empty in-memory shell.
fn build_base(
    project_root: Option<&Path>,
) -> Result<(
    AnalysisPackage,
    CompiledModule,
    HashMap<NameKey, blake3::Hash>,
)> {
    match project_root {
        Some(root) => {
            let stubs = ambient_platform::stub_natives();
            let result = ambient_engine::build::build_package(
                root,
                crate::commands::parse_source,
                &BuildOptions {
                    platform_modules: ambient_platform::platform_modules(),
                    natives: Some(&stubs),
                    progress: None,
                    // Warm the base build off a prior `ambient run`/`compile`
                    // snapshot: REPL startup on a built project skips
                    // recompiling unchanged modules. The REPL is a read-only
                    // cache *consumer* — it never writes a snapshot. A REPL
                    // session is not a canonical build (its accumulating `repl`
                    // module and per-turn trial compiles are ephemeral), so
                    // persisting one would only churn the store the real build
                    // owns; the per-turn trial compiles stay uncached as before.
                    store_path: Some(ambient_engine::disk_store::DiskStore::package_store_path(
                        root,
                    )),
                    ..Default::default()
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
pub enum ReplCommand {
    Help,
    Quit,
    Clear,
    Unknown(String),
}

/// Parse a REPL command line (leading `:` already present).
#[must_use]
pub fn parse_repl_command(line: &str) -> ReplCommand {
    let cmd = line.trim_start_matches(':').split_whitespace().next();
    match cmd {
        Some("help") | Some("h") | Some("?") => ReplCommand::Help,
        Some("quit") | Some("q") | Some("exit") => ReplCommand::Quit,
        Some("clear") | Some("reset") => ReplCommand::Clear,
        Some(other) => ReplCommand::Unknown(other.to_string()),
        None => ReplCommand::Unknown(String::new()),
    }
}

/// Write the REPL help text line by line to `emit`. Shared by both frontends
/// so the help content lives in exactly one place.
pub fn write_repl_help(emit: &dyn Fn(&str)) {
    emit("REPL Commands:");
    emit("  :help, :h, :?    Show this help message");
    emit("  :quit, :q, :exit Exit the REPL");
    emit("  :clear, :reset   Clear all defined functions and variables");
    emit("");
    emit("Key Bindings:");
    emit("  Ctrl+E, Ctrl+O   Edit current line in $EDITOR");
    emit("");
    emit("Definitions:");
    emit("  fn add(x, y) { x + y }   Define a function");
    emit("  const PI: Number = 3      Define a constant (must be a literal)");
    emit("  struct Point { x: Number, y: Number }   Define a type");
    emit("  use core::system::Stdio   Import across turns");
    emit("");
    emit("Expressions:");
    emit("  1 + 2 * 3        Evaluate an expression");
    emit("  add(1, 2)        Call a defined function");
    emit("  \"hello\"          String literal");
    emit("");
    emit("The REPL runs the full pipeline: type checking, every item kind");
    emit("(struct/enum/type/ability/trait/impl/use), cross-module `use`, and");
    emit("the same diagnostics as `ambient check`. A `const` must be a literal;");
    emit("use a `fn` for a computed value.");
    emit("");
    emit("Every turn is a deploy: redefining a name live-upgrades the running");
    emit("program — a task ensured earlier picks the new code up on its next");
    emit("pass. A rejected deploy (a failed State migration check) errors the");
    emit("turn and leaves the program untouched.");
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
