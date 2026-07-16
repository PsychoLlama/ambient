//! The REPL session: an exploration surface over a project's running code.
//!
//! A session is *not* an authoring surface. Definitions (functions, types,
//! traits, abilities, impls) belong in module files — `ambient dev` and the
//! REPL's own live reload pick them up as they are saved. What a session
//! accumulates is exploration state:
//!
//! - **`use` imports**, re-emitted into every turn so names stay in scope.
//! - **Bindings** (`x = expr`): the expression is evaluated once and its
//!   value saved under the name. Later turns receive saved bindings as
//!   arguments to the turn's synthetic entry function, whose parameter
//!   types the checker pins to the bindings' recorded types — a binding is
//!   a real lexical local in every later turn, shadowing module names
//!   exactly as the language's scoping rules say locals do.
//!
//! The session's scope is anchored where the REPL was started: the virtual
//! session module lives at `<cwd>/__repl.ab` (clamped into the package's
//! source directory), so `pkg`, `self`, and `super` resolve exactly as they
//! would in a file authored in that directory. Outside a project the session
//! is a bare single-module package: `pkg`/`self`/`super` have nothing to
//! refer to, and the diagnostics say so.
//!
//! Every expression turn is still a deploy ([`RuntimeHost::deploy_incremental`])
//! onto one long-lived host, so effects run with the full platform set and
//! tasks ensured in earlier turns keep running across turns.
//!
//! This is the engine behind both frontends: the interactive [`cmd_repl`]
//! rustyline loop (`super`) and the in-process integration-test harness
//! (`crates/ambient-cli/tests/repl_harness`). It owns "what a turn does"
//! and routes every out-of-band line — deploy notes/warnings, task
//! lifecycle events, `State cleared.` — through an injectable [`ReplIo`],
//! plus a [`StdioSink`] for program (`Stdio`/`Log`) output.

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
use ambient_engine::types::Type;
use ambient_engine::value::{ModuleExport, ModuleExportKind, ModuleMemberRef, ModuleValue, Value};
use ambient_parser::ReplInput;
use ambient_platform::{StdioSink, TaskEvent, TaskEventSink};

use crate::commands::core_context;
use crate::commands::host::{HostDeployOutcome, RuntimeHost};
use crate::diagnostic::report_build_error;

/// The reserved leaf name of the virtual session module: the session checks
/// and compiles as if a file `__repl.ab` sat in the launch directory.
const SESSION_MODULE: &str = "__repl";

/// The prefix of every turn's synthetic entry function. Never shown to the
/// user: [`scrub_entry_names`] rewrites it out of every rendered error.
const ENTRY_PREFIX: &str = "__repl_entry_";

/// The message emitted to the control sink after a successful `:clear`.
pub const STATE_CLEARED: &str = "State cleared.";

/// A control-line sink: the target for everything the session would print
/// to a terminal's stderr — deploy notes/warnings, task lifecycle events,
/// binding announcements, and `State cleared.`.
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

/// One committed `use` import: the names it binds (for last-wins
/// replacement) plus its raw source, re-emitted verbatim each turn.
struct UseEntry {
    /// The terminal names this import binds in the session module.
    names: Vec<Arc<str>>,
    /// The raw source text of the input.
    source: String,
}

/// One saved session binding: the value a `name = expr` turn produced and
/// the checked type it was produced at.
struct Binding {
    /// The bound name — a parameter of every later turn's entry function.
    name: Arc<str>,
    /// The expression's resolved type, seeded into later checks.
    ty: Type,
    /// The value, passed to later entries by position.
    value: Value,
}

/// Everything the REPL accumulates and reuses across turns.
pub struct ReplSession {
    /// Committed `use` imports, in insertion order.
    uses: Vec<UseEntry>,
    /// Saved bindings, in insertion order (rebinding replaces in place).
    bindings: Vec<Binding>,
    /// The virtual session module's path, anchored at the launch directory.
    session_path: ModulePath,
    /// The analysis package: an opened project, or an empty in-memory
    /// package when the REPL runs outside a project.
    package: AnalysisPackage,
    /// The project root, if any — used to rebuild base state on `:clear`
    /// and `:reload`.
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
    /// anchor the session module at `launch_dir`, and start the runtime
    /// host wired to `io`.
    pub fn new(launch_dir: &Path, io: ReplIo) -> Result<Self> {
        let project_root = find_project_root(launch_dir);
        let (package, base, imported_hashes) = build_base(project_root.as_deref())?;
        let session_path = session_module_path(&package, launch_dir);
        // The REPL has no program args; `Env::args!()` is empty.
        let host = RuntimeHost::new(
            task_event_sink(Arc::clone(&io.control)),
            io.program.clone(),
            Vec::new(),
        )?;

        Ok(Self {
            uses: Vec::new(),
            bindings: Vec::new(),
            session_path,
            package,
            project_root,
            base,
            imported_hashes,
            entry_counter: 0,
            host,
            io,
        })
    }

    /// Emit a control line (a deploy note, a task event, …).
    fn control(&self, line: &str) {
        (self.io.control)(line);
    }

    /// The project root the session was opened in, if any — what a
    /// frontend watches for live reload.
    #[must_use]
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Wind the running program down: drain every task and wait for it to
    /// stop, so a lingering ticker can't print into a dropped buffer or
    /// keep the test process alive.
    pub fn shutdown(&self) {
        self.host.tasks().drain_all();
        self.host.tasks().wait_all();
    }

    /// Reset all session state (imports, bindings) and rebuild the base +
    /// host from scratch, then announce `State cleared.` on the control
    /// sink.
    ///
    /// The running program winds down first: tasks are drained and waited
    /// for (bounded by the drain deadline), so a lingering ticker can't
    /// keep printing into the fresh session. Errors are returned for the
    /// frontend to render.
    pub fn clear(&mut self) -> Result<()> {
        self.shutdown();
        let (package, base, imported_hashes) = build_base(self.project_root.as_deref())?;
        self.uses.clear();
        self.bindings.clear();
        self.package = package;
        self.base = base;
        self.imported_hashes = imported_hashes;
        self.entry_counter = 0;
        self.host = RuntimeHost::new(
            task_event_sink(Arc::clone(&self.io.control)),
            self.io.program.clone(),
            Vec::new(),
        )?;
        // Drop any lingering session module from the package.
        self.sync_session_module(&self.uses_source());
        self.control(STATE_CLEARED);
        Ok(())
    }

    /// Rebuild the base from the project's current on-disk sources, keeping
    /// the session's imports, bindings, and running tasks. The freshly
    /// built base deploys immediately, so running tasks pick the new code
    /// up on their next pass — this is how file edits made while the REPL
    /// is open become visible.
    ///
    /// A broken build reports its diagnostics and leaves the previous base
    /// (and the running program) untouched.
    pub fn reload(&mut self) -> Result<()> {
        let Some(root) = self.project_root.clone() else {
            self.control("note: not in a project; nothing to reload");
            return Ok(());
        };
        let (package, base, imported_hashes) = build_base(Some(&root))?;
        self.package = package;
        self.base = base;
        self.imported_hashes = imported_hashes;
        self.sync_session_module(&self.uses_source());

        // Ship the fresh base so running tasks rebind without waiting for
        // the next expression turn. The entry is a synthetic no-op.
        self.entry_counter += 1;
        let entry_local = format!("{ENTRY_PREFIX}{}", self.entry_counter);
        let uses = self.uses_source();
        let trial_source = format!("{uses}fn {entry_local}() {{ }}\n");
        let (registry, check) = self
            .check_trial(&trial_source, &entry_local, &[])
            .map_err(|e| anyhow!("session no longer checks after reload:\n{e}"))?;
        let merged = self.compile_trial(&registry, &check, &trial_source)?;
        self.sync_session_module(&uses);
        let entry_qualified = format!("{}::{entry_local}", self.session_path);
        let outcome = self
            .host
            .deploy_incremental(&merged, &entry_qualified, Vec::new())?;
        self.report_deploy(&outcome);
        self.control("Reloaded.");
        Ok(())
    }

    /// Execute a parsed REPL command. [`ReplCommand::Quit`] is a no-op here
    /// (the frontend decides how to exit); the rest write through the
    /// control sink. A failed `:clear`/`:reload` is returned for the
    /// frontend to render.
    pub fn run_command(&mut self, command: ReplCommand) -> Result<()> {
        match command {
            ReplCommand::Help => {
                let control = Arc::clone(&self.io.control);
                write_repl_help(&|line| control(line));
                Ok(())
            }
            ReplCommand::Clear => self.clear(),
            ReplCommand::Reload => self.reload(),
            ReplCommand::Quit => Ok(()),
            ReplCommand::Unknown(cmd) => {
                self.control(&format!("Unknown command: {cmd}"));
                self.control("Type :help for available commands.");
                Ok(())
            }
        }
    }

    /// The concatenated source of every committed `use` import.
    fn uses_source(&self) -> String {
        let mut source = String::new();
        for entry in &self.uses {
            source.push_str(&entry.source);
            source.push('\n');
        }
        source
    }

    /// Install `source` as the current session module in the analysis
    /// package.
    fn sync_session_module(&mut self, source: &str) {
        self.package
            .insert_module(self.session_path.clone(), source.to_string());
    }

    /// Evaluate one line: an inspection query, a `use` import, a binding,
    /// or an expression. Expressions and bindings return their value
    /// (unless `Unit`); every rendered error has the synthetic entry name
    /// scrubbed out.
    pub fn eval(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
        self.eval_inner(line)
            .map_err(|e| self.annotate_error(line, &e))
    }

    /// [`Self::eval`] before error-rendering cleanup.
    fn eval_inner(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
        let trimmed = line.trim();

        // Inspection: a `::` path (or bare module name) that names a
        // module or one of its members browses the registry instead of
        // evaluating. Namespaces are addressed with `::`; a `.` is
        // value/field access and never resolves a namespace, so a dotted
        // path falls through to the parser (which rejects it). A bare
        // name that matches a session binding always evaluates — locals
        // shadow module names. The path-shape guard keeps ordinary
        // expressions (`1 + 2`, `f(x)`) off the registry-building path.
        if looks_like_path(trimmed)
            && !self.bindings.iter().any(|b| &*b.name == trimmed)
            && let Some(value) = self.introspect(trimmed)
        {
            return Ok(Some(value));
        }

        // Parse the input as items, a binding, or an expression.
        let input = match ambient_parser::parse_repl_input(line) {
            Ok(i) => i,
            Err(e) => return Err(format_repl_parse_error(line, &e)),
        };

        match input {
            ReplInput::Items(items) => self.eval_items(line, &items),
            ReplInput::Expr(_) => self.eval_expression(line),
            ReplInput::Binding { name, .. } => self.eval_binding(line, &name),
        }
    }

    /// Commit a `use` import after checking it against the session module.
    /// Any other item kind is an authoring action and errors: definitions
    /// live in module files, which the session reloads live.
    fn eval_items(
        &mut self,
        line: &str,
        items: &[Item],
    ) -> std::result::Result<Option<Value>, String> {
        if let Some(item) = items.iter().find(|i| !matches!(i.kind, ItemKind::Use(_))) {
            return Err(definitions_unsupported(item));
        }

        // Check the import against the session module (an import can fail:
        // missing module, missing symbol, private symbol). No deploy — an
        // import introduces no code.
        self.entry_counter += 1;
        let entry_local = format!("{ENTRY_PREFIX}{}", self.entry_counter);
        let existing = self.uses_source();
        let trial_source = format!("{existing}{line}\nfn {entry_local}() {{ }}\n");
        self.check_trial(&trial_source, &entry_local, &[])?;

        // Commit: drop earlier imports this input re-binds, then append.
        let names: Vec<Arc<str>> = items.iter().filter_map(use_bound_name).collect();
        self.uses.retain(|existing| {
            existing.source != line && !existing.names.iter().any(|n| names.contains(n))
        });
        self.uses.push(UseEntry {
            names,
            source: line.to_string(),
        });
        self.sync_session_module(&self.uses_source());
        Ok(None)
    }

    /// Evaluate an expression turn and return its value.
    fn eval_expression(&mut self, line: &str) -> std::result::Result<Option<Value>, String> {
        let (value, _) = self.eval_entry(line)?;
        if matches!(value, Value::Unit) {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    /// Evaluate a binding turn: run the initializer once, then save the
    /// value under `name` with its checked type. Later turns see the
    /// binding as a lexical local.
    fn eval_binding(
        &mut self,
        line: &str,
        name: &Arc<str>,
    ) -> std::result::Result<Option<Value>, String> {
        // The parser guarantees the shape `<ident> = <expr>`, so the text
        // after the first `=` is exactly the initializer.
        let expr_src = line
            .split_once('=')
            .map(|(_, rest)| rest.trim())
            .ok_or_else(|| "malformed binding".to_string())?;

        let (value, ty) = self.eval_entry(expr_src)?;
        let ty = ty.ok_or_else(|| format!("cannot save `{name}`: no type for the expression"))?;

        // A binding must have a fully determined type: its value is passed
        // into later turns as an entry argument, and an undetermined
        // position would let a later use dictate a type the value doesn't
        // have.
        if !ty.free_vars().is_empty() {
            return Err(format!(
                "cannot save `{name}`: its type `{}` is not fully determined \
                 — give the expression a concrete type",
                ambient_analysis::queries::format_type(&ty)
            ));
        }

        self.control(&format!(
            "{name}: {}",
            ambient_analysis::queries::format_type(&ty)
        ));
        let echo = if matches!(value, Value::Unit) {
            None
        } else {
            Some(value.clone())
        };
        let binding = Binding {
            name: Arc::clone(name),
            ty,
            value,
        };
        match self.bindings.iter_mut().find(|b| b.name == binding.name) {
            Some(existing) => *existing = binding,
            None => self.bindings.push(binding),
        }
        Ok(echo)
    }

    /// Check, compile, and run one expression as a synthetic entry function
    /// whose parameters are the session's saved bindings. Returns the
    /// entry's value and the expression's resolved type.
    fn eval_entry(&mut self, expr_src: &str) -> std::result::Result<(Value, Option<Type>), String> {
        self.entry_counter += 1;
        let entry_local = format!("{ENTRY_PREFIX}{}", self.entry_counter);
        let uses = self.uses_source();

        // The entry takes every saved binding as a parameter — the checker
        // pins each parameter to the binding's recorded type, and the host
        // passes the saved values positionally. The entry is private and
        // unannotated, so effect inference gives it whatever abilities the
        // expression performs — no `with` clause or `use` needed for
        // fully-qualified platform calls.
        let params: Vec<&str> = self.bindings.iter().map(|b| &*b.name).collect();
        let param_types: Vec<Type> = self.bindings.iter().map(|b| b.ty.clone()).collect();
        let trial_source = format!(
            "{uses}fn {entry_local}({}) {{\n{expr_src}\n}}\n",
            params.join(", ")
        );

        let (registry, check) = self.check_trial(&trial_source, &entry_local, &param_types)?;
        let merged = self
            .compile_trial(&registry, &check, &trial_source)
            .map_err(|e| format!("{e}"))?;

        // The synthetic entry is never committed; restore the package's
        // session module to the committed state.
        self.sync_session_module(&uses);

        let entry_qualified = format!("{}::{entry_local}", self.session_path);
        let args: Vec<Value> = self.bindings.iter().map(|b| b.value.clone()).collect();
        let outcome = self
            .host
            .deploy_incremental(&merged, &entry_qualified, args)
            .map_err(|e| format!("{e}"))?;
        self.report_deploy(&outcome);

        Ok((outcome.report.value, check.entry_type))
    }

    /// Type-check a trial session module once and reject the turn if it
    /// produces any diagnostics. On success the built registry *and* the
    /// single [`SessionCheck`](ambient_analysis::SessionCheck) (its typed AST
    /// and canonical signatures) are returned, so the caller compiles from the
    /// same check instead of re-running inference — a turn type-checks exactly
    /// once.
    fn check_trial(
        &mut self,
        trial_source: &str,
        entry_local: &str,
        param_types: &[Type],
    ) -> std::result::Result<(ModuleRegistry, ambient_analysis::SessionCheck), String> {
        self.sync_session_module(trial_source);
        let registry = self.package.build_registry();
        let entry_spec = ambient_engine::infer::SessionEntrySpec {
            entry: entry_local,
            param_types,
        };
        let check = ambient_analysis::check_session_module(
            trial_source,
            &self.session_path,
            &registry,
            &entry_spec,
        );
        let diagnostics = check.diagnostics();
        if diagnostics.is_empty() {
            Ok((registry, check))
        } else {
            // Leave the committed module in place for the next turn.
            let committed = self.uses_source();
            self.sync_session_module(&committed);
            Err(format_repl_diagnostics(trial_source, &diagnostics))
        }
    }

    /// Compile the trial session module — reusing the turn's single
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
            &self.session_path,
            trial_source,
            self.imported_hashes.clone(),
        )
        .map_err(report_build_error)
    }

    /// Browse the registry for a module path or member path, if `path` names
    /// one. Bare value names (variables, constants, functions) return `None`
    /// so they evaluate normally.
    fn introspect(&self, path: &str) -> Option<Value> {
        let registry = self.package.build_registry();

        // Whole module: `core`, `core::collections::list`.
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

    /// Final error-rendering cleanup for a turn: scrub the synthetic entry
    /// name, and when the session has no project, explain why `pkg`/`self`/
    /// `super` have nothing to refer to.
    fn annotate_error(&self, line: &str, error: &str) -> String {
        let mut rendered = scrub_entry_names(error);
        if self.project_root.is_none() && mentions_project_roots(line) {
            rendered.push_str(
                "\n  note: the REPL was started outside an ambient package \
                 (no ambient.toml found), so `pkg`, `self`, and `super` \
                 have nothing to refer to",
            );
        }
        rendered
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
                    // Warm the base build off a prior `ambient run`/`build`
                    // snapshot: REPL startup on a built project skips
                    // recompiling unchanged modules. The REPL is a read-only
                    // cache *consumer* — it never writes a snapshot. A REPL
                    // session is not a canonical build (its per-turn trial
                    // compiles are ephemeral), so persisting one would only
                    // churn the store the real build owns.
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

/// The virtual session module's path: `<launch_dir>/__repl.ab` mapped
/// through the package's file↔module convention, so `pkg`/`self`/`super`
/// resolve exactly as they would in a file authored where the REPL was
/// started. A launch directory outside the package's source tree (the
/// project root, or no project at all) anchors at the source root.
fn session_module_path(package: &AnalysisPackage, launch_dir: &Path) -> ModulePath {
    let virtual_file = ambient_analysis::package::lexically_normalize(
        &launch_dir.join(format!("{SESSION_MODULE}.ab")),
    );
    package
        .module_path_for(&virtual_file)
        .or_else(|| ModulePath::from_str_segments(&[SESSION_MODULE]))
        .expect("the reserved session module name is a valid module path")
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

/// The terminal name a `use` item binds, for last-wins replacement.
fn use_bound_name(item: &Item) -> Option<Arc<str>> {
    match &item.kind {
        ItemKind::Use(def) => def
            .alias
            .as_ref()
            .map(|(name, _)| name.clone())
            .or_else(|| def.path.last().map(|(name, _)| name.clone())),
        _ => None,
    }
}

/// The error for a definition entered at the prompt: the REPL is an
/// exploration surface, not an authoring one.
fn definitions_unsupported(item: &Item) -> String {
    let what = match &item.kind {
        ItemKind::Function(_) => "a function",
        ItemKind::Const(_) => "a constant",
        ItemKind::Struct(_) => "a struct",
        ItemKind::TypeAlias(_) => "a type alias",
        ItemKind::Enum(_) => "an enum",
        ItemKind::Ability(_) => "an ability",
        ItemKind::Trait(_) => "a trait",
        ItemKind::ExternFn(_) => "an extern fn",
        ItemKind::Set(_) => "an ability set",
        ItemKind::Impl(_) => "an impl",
        ItemKind::Use(_) => "an import",
    };
    format!(
        "the REPL doesn't host definitions: write {what} in a module file \
         instead — the session picks up saved changes (`:reload` forces it).\n\
         The REPL evaluates expressions, saves bindings (`x = expr`), and \
         imports names (`use …`)."
    )
}

/// Map a registry export kind to its value-level rendering kind.
fn export_kind(kind: ExportKind) -> ModuleExportKind {
    match kind {
        ExportKind::Function => ModuleExportKind::Function,
        ExportKind::Const => ModuleExportKind::Const,
        ExportKind::Struct | ExportKind::TypeAlias => ModuleExportKind::Type,
        ExportKind::Enum => ModuleExportKind::Enum,
        ExportKind::EnumVariant => ModuleExportKind::Variant,
        // Ability methods are never module-level exports; render one like
        // its ability if it ever appears here.
        ExportKind::Ability | ExportKind::Trait | ExportKind::AbilityMethod | ExportKind::Set => {
            ModuleExportKind::Ability
        }
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

/// Whether the input references any of the project-relative path roots.
fn mentions_project_roots(line: &str) -> bool {
    ["pkg", "self", "super"].iter().any(|root| {
        line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|word| word == *root)
    })
}

/// Rewrite the synthetic entry name (`__repl_entry_7`) out of rendered
/// errors: the wrapper is an implementation detail, and a note like
/// ``in function `__repl_entry_7` `` should read as "in this input".
fn scrub_entry_names(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(ENTRY_PREFIX) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + ENTRY_PREFIX.len()..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        out.push_str("repl input");
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// REPL command types.
pub enum ReplCommand {
    Help,
    Quit,
    Clear,
    Reload,
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
        Some("reload") | Some("r") => ReplCommand::Reload,
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
    emit("  :clear, :reset   Clear imports, bindings, and the running program");
    emit("  :reload, :r      Rebuild the project from disk and redeploy");
    emit("");
    emit("Key Bindings:");
    emit("  Ctrl+E, Ctrl+O   Edit current line in $EDITOR");
    emit("");
    emit("The REPL is for exploring code, not authoring it:");
    emit("  1 + 2 * 3            Evaluate an expression");
    emit("  parse(\"[1, 2]\")      Call project and core functions");
    emit("  xs = List::of(1, 2)  Save a value; later turns see `xs`");
    emit("  use pkg::utils;      Import for the rest of the session");
    emit("  core::option         Inspect a module");
    emit("");
    emit("Definitions (fn/struct/trait/ability/impl/…) live in module files.");
    emit("Saved file changes flow into the session (`:reload` forces it);");
    emit("tasks pick rebound names up on their next pass.");
    emit("");
    emit("The session's scope is the directory the REPL was started in:");
    emit("`pkg`, `self`, and `super` resolve as they would in a file there.");
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
/// The trial source is `committed imports + this turn's input`, so a caret
/// can point at the exact offending line even when the error is in the
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
