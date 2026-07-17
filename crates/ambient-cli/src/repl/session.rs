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

use anyhow::{Result, anyhow, bail};

use ambient_analysis::package::AnalysisPackage;
use ambient_engine::ast::{Item, ItemKind};
use ambient_engine::build::{BuildOptions, compile_session_module};
use ambient_engine::compiler::CompiledModule;
use ambient_engine::fqn::NameKey;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_engine::types::Type;
use ambient_engine::value::{ModuleMemberRef, Value};
use ambient_parser::ReplInput;
use ambient_platform::{StdioSink, TaskEvent, TaskEventSink};

use super::inspect::{
    display_path, export_kind, inspect_signature, looks_like_path, module_listing,
};
use super::render::{
    definitions_unsupported, format_repl_diagnostics, format_repl_parse_error,
    mentions_project_roots, scrub_entry_names,
};
use crate::commands::core_context;
use crate::commands::host::{HostDeployOutcome, RuntimeHost};
use crate::diagnostic::report_build_error;

pub use super::render::{ReplCommand, parse_repl_command, write_repl_help};

/// The reserved leaf name of the virtual session module: the session checks
/// and compiles as if a file `__repl.ab` sat in the launch directory.
const SESSION_MODULE: &str = "__repl";

/// The prefix of every turn's synthetic entry function. Never shown to the
/// user: [`scrub_entry_names`] rewrites it out of every rendered error.
pub(crate) const ENTRY_PREFIX: &str = "__repl_entry_";

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
    /// Each bound name's target path as spelled (prefix words included),
    /// so inspection can expand an imported alias to its full path.
    targets: Vec<(Arc<str>, Vec<String>)>,
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
    /// control sink. A failed command is returned for the frontend to
    /// render.
    pub fn run_command(&mut self, command: ReplCommand) -> Result<()> {
        match command {
            ReplCommand::Help => {
                let control = Arc::clone(&self.io.control);
                write_repl_help(&|line| control(line));
                Ok(())
            }
            ReplCommand::Clear => self.clear(),
            ReplCommand::Reload => self.reload(),
            ReplCommand::Sig(path) => self.show_signature(&path),
            ReplCommand::Type(expr) => self.show_type(&expr),
            ReplCommand::Quit => Ok(()),
            ReplCommand::Unknown(cmd) => {
                self.control(&format!("Unknown command: {cmd}"));
                self.control("Type :help for available commands.");
                Ok(())
            }
        }
    }

    /// `:sig <path>`: show a module member's declaration signature and doc
    /// (or a binding's type). Unlike bare-path inspection this also reaches
    /// names a binding shadows.
    fn show_signature(&mut self, path: &str) -> Result<()> {
        let path = path.trim();
        if path.is_empty() {
            bail!("usage: :sig <path> — e.g. `:sig core::option::flatten`");
        }
        if let Some(binding) = self.bindings.iter().find(|b| &*b.name == path) {
            self.control(&format!(
                "{path}: {}",
                ambient_analysis::queries::format_type(&binding.ty)
            ));
            return Ok(());
        }
        if !looks_like_path(path) {
            bail!("`:sig` takes a `::` path or a binding name, got `{path}`");
        }
        match self.introspect(path) {
            Some(value) => {
                let rendered = ambient_engine::format::format_value(&value);
                for line in rendered.lines() {
                    self.control(line);
                }
                Ok(())
            }
            None => bail!("nothing in scope is named `{path}`"),
        }
    }

    /// `:type <expr>`: check the expression against the session (bindings
    /// in scope) and show its inferred type without running it.
    fn show_type(&mut self, expr_src: &str) -> Result<()> {
        let expr_src = expr_src.trim();
        if expr_src.is_empty() {
            bail!("usage: :type <expr> — e.g. `:type [1, 2].contains(1)`");
        }
        self.entry_counter += 1;
        let entry_local = format!("{ENTRY_PREFIX}{}", self.entry_counter);
        let uses = self.uses_source();
        let params: Vec<&str> = self.bindings.iter().map(|b| &*b.name).collect();
        let param_types: Vec<Type> = self.bindings.iter().map(|b| b.ty.clone()).collect();
        let trial_source = format!(
            "{uses}fn {entry_local}({}) {{\n{expr_src}\n}}\n",
            params.join(", ")
        );
        let checked = self.check_trial(&trial_source, &entry_local, &param_types);
        self.sync_session_module(&uses);
        let (_, check) = checked.map_err(|e| anyhow!("{}", self.annotate_error(expr_src, &e)))?;
        let ty = check
            .entry_type
            .ok_or_else(|| anyhow!("no type could be inferred"))?;
        self.control(&ambient_analysis::queries::format_type(&ty));
        Ok(())
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
        let targets: Vec<(Arc<str>, Vec<String>)> = items
            .iter()
            .filter_map(|item| {
                let (name, target) = use_target(item)?;
                Some((name, target))
            })
            .collect();
        self.uses.retain(|existing| {
            existing.source != line && !existing.names.iter().any(|n| names.contains(n))
        });
        self.uses.push(UseEntry {
            names,
            source: line.to_string(),
            targets,
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

    /// Browse the registry for the module or member `path` names. Paths
    /// resolve the way the language resolves them: `pkg`, `self`, and
    /// `super` anchor at the session module (exactly as in a file at the
    /// launch directory); anything else is an absolute path (`core::…`, or
    /// a package module by its root-relative path). Bare value names
    /// return `None` so they evaluate normally.
    ///
    /// What shows is what the session can access: a module lists its `pub`
    /// exports and child modules; a member shows its declaration signature
    /// and doc. Private items don't show — inspection answers "what can I
    /// call from here", not "what does the file contain".
    fn introspect(&self, path: &str) -> Option<Value> {
        let registry = self.package.build_registry();
        let mut segments: Vec<String> = path.split("::").map(str::to_string).collect();

        // Expand a leading `use`-bound alias to the path it was imported
        // from (`use self::client;` makes `client::port` inspectable), a
        // few levels deep in case an alias chains through another.
        for _ in 0..4 {
            let Some(first) = segments.first().cloned() else {
                break;
            };
            let Some(target) = self
                .uses
                .iter()
                .flat_map(|u| &u.targets)
                .find(|(name, _)| **name == *first)
            else {
                break;
            };
            let mut expanded: Vec<String> = target.1.clone();
            expanded.extend(segments.drain(1..));
            segments = expanded;
        }

        let seg_refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        if let Some(value) = self.introspect_absolute(&registry, &seg_refs) {
            return Some(value);
        }

        // A bare name may be a prelude item (`Number`, `Option`, `Show`):
        // every module sees the prelude's re-exports without an import.
        if let [name] = seg_refs[..] {
            return self.introspect_absolute(&registry, &["core", "prelude", name]);
        }
        None
    }

    /// Resolve and render one spelled path (roots not yet resolved).
    fn introspect_absolute(&self, registry: &ModuleRegistry, segments: &[&str]) -> Option<Value> {
        let absolute = self.resolve_inspect_path(segments)?;

        // Whole module: a file module, a directory namespace, or the
        // package root (`pkg`).
        if let Some(value) = module_listing(
            registry,
            &absolute,
            &self.session_path,
            &self.package_root_segments(),
        ) {
            return Some(value);
        }

        // Member: the trailing name inside its parent module, resolved
        // through the registry's canonical lookup (which chases `pub use`
        // re-export chains — `core::system::Stdio` really lives in
        // `core::system::stdio` — and enforces visibility, so a private
        // item reads as absent exactly like calling it would).
        let (member, parent) = absolute.split_last()?;
        let parent_path = ModulePath::from_segments(parent.to_vec())?;
        let (export, defining) = registry.lookup_symbol(&parent_path, member).ok()?;
        let signature = registry.get(&defining).and_then(|info| {
            info.module
                .items
                .iter()
                .find(|item| {
                    ambient_analysis::queries::item_name(item).is_some_and(|n| *n == export.name)
                })
                .map(|item| Arc::from(inspect_signature(item)))
        });
        // Show where the item actually lives: a re-export chased through
        // `pub use` (or the prelude) renders its defining module.
        let mut shown = display_path(defining.segments(), &self.package_root_segments());
        shown.push_str("::");
        shown.push_str(&export.name);
        Some(Value::ModuleMember(Arc::new(ModuleMemberRef {
            path: shown.into(),
            kind: export_kind(export.kind),
            signature,
            doc: export.doc.clone(),
        })))
    }

    /// Resolve an inspection path's leading root against the session
    /// module: `pkg`/`self`/`super` anchor exactly as a `use` in a file at
    /// the launch directory would; anything else is already absolute.
    /// Returns the absolute registry segments (empty = the package root).
    /// The session's package mount segments (`["probe"]` for a project
    /// named `probe`): where `pkg` anchors and the floor `super` may not
    /// step above. Empty for a project-less session (bare layout).
    fn package_root_segments(&self) -> Vec<Arc<str>> {
        if self.package.package_name().is_empty() {
            Vec::new()
        } else {
            vec![Arc::from(self.package.package_name())]
        }
    }

    fn resolve_inspect_path(&self, segments: &[&str]) -> Option<Vec<Arc<str>>> {
        use ambient_engine::module_path::ImportPrefix;
        let to_arcs =
            |segs: &[&str]| -> Vec<Arc<str>> { segs.iter().map(|s| Arc::from(*s)).collect() };
        let package_root = self.package_root_segments();

        let prefix = match *segments.first()? {
            "pkg" => ImportPrefix::Pkg,
            "self" => ImportPrefix::Self_,
            "super" => {
                let count = segments.iter().take_while(|s| **s == "super").count();
                ImportPrefix::Super(count)
            }
            _ => return Some(to_arcs(segments)),
        };
        let rest = match &prefix {
            ImportPrefix::Super(count) => &segments[*count..],
            _ => &segments[1..],
        };

        if rest.is_empty() {
            // A bare root names the anchor itself: `pkg` is the package
            // root (the mount), `self` the launch directory, `super` its
            // parent — never above the mount.
            let dir = self.session_path.file_dir(false);
            return match prefix {
                ImportPrefix::Pkg => Some(package_root),
                ImportPrefix::Self_ => Some(dir.map_or_else(Vec::new, |d| d.segments().to_vec())),
                ImportPrefix::Super(count) => {
                    let mut segs = dir.map_or_else(Vec::new, |d| d.segments().to_vec());
                    for _ in 0..count {
                        if segs.len() <= package_root.len() {
                            return None;
                        }
                        segs.pop();
                    }
                    Some(segs)
                }
                // Unreachable: only pkg/self/super map to a prefix above.
                ImportPrefix::Core | ImportPrefix::Workspace => None,
            };
        }

        self.session_path
            .resolve_relative(&prefix, &to_arcs(rest), false, &package_root)
            .ok()
            .map(|p| p.segments().to_vec())
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
            let package = AnalysisPackage::empty(PathBuf::from("."), PathBuf::from("."), "");
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
        .or_else(|| {
            // Outside the source tree (or with no project): anchor at the
            // source root. Module paths are mounted under the package name,
            // so the fallback must be too — a bare path in a mounted
            // registry would resolve `pkg::`/`self` at the workspace root.
            if package.package_name().is_empty() {
                ModulePath::from_str_segments(&[SESSION_MODULE])
            } else {
                ModulePath::from_str_segments(&[package.package_name(), SESSION_MODULE])
            }
        })
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

/// The bound name and target path (as spelled, prefix words included) of a
/// `use` item — how inspection expands an imported alias.
fn use_target(item: &Item) -> Option<(Arc<str>, Vec<String>)> {
    use ambient_engine::ast::UsePrefix;
    let ItemKind::Use(def) = &item.kind else {
        return None;
    };
    let name = use_bound_name(item)?;
    let mut target: Vec<String> = match &def.prefix {
        UsePrefix::Pkg => vec!["pkg".to_string()],
        UsePrefix::Core => vec!["core".to_string()],
        UsePrefix::Self_ => vec!["self".to_string()],
        UsePrefix::Super(count) => vec!["super".to_string(); *count],
        // An empty leading word renders the workspace root's bare `::`
        // when the segments are joined (`::other_pkg::thing`).
        UsePrefix::Workspace => vec![String::new()],
        UsePrefix::Local => Vec::new(),
    };
    target.extend(def.path.iter().map(|(seg, _)| seg.to_string()));
    Some((name, target))
}

/// Everything the interactive completer needs to answer completion queries
/// for the current session state, snapshotted so the rustyline helper can
/// own it across readline calls without borrowing the session.
///
/// The snapshot pre-renders the trial-source *prefix* — the committed `use`
/// imports plus a synthetic entry-fn header whose parameters are the saved
/// bindings — so the completer only appends the in-progress line and a
/// closing brace to reconstruct the module shape a real turn checks, and
/// maps cursor offsets by adding `prefix.len()`.
pub struct CompletionSnapshot {
    /// The trial source up to and including the newline that opens the
    /// synthetic entry function's body.
    pub prefix: String,
    /// The registry the committed session module is registered in — the
    /// same one a turn checks against ([`AnalysisPackage::build_registry`]).
    pub registry: ModuleRegistry,
    /// The virtual session module's identity in that registry.
    pub session_path: ModulePath,
}

impl ReplSession {
    /// Snapshot the session's completion inputs (see [`CompletionSnapshot`]).
    ///
    /// Binding parameters are spelled with their recorded types, so the
    /// checker — and with it dot-member completion on a binding — sees the
    /// types a real turn pins through its session entry spec. Completion is
    /// best-effort where evaluation is exact: a type whose display form
    /// doesn't round-trip through the parser (or names something not in
    /// scope here) degrades that one binding to an untyped parameter rather
    /// than breaking the whole trial parse.
    #[must_use]
    pub fn completion_snapshot(&self) -> CompletionSnapshot {
        let params: Vec<String> = self
            .bindings
            .iter()
            .map(|b| annotated_param(&b.name, &b.ty))
            .collect();
        let prefix = format!(
            "{}fn __repl_complete({}) {{\n",
            self.uses_source(),
            params.join(", ")
        );
        CompletionSnapshot {
            prefix,
            registry: self.package.build_registry(),
            session_path: self.session_path.clone(),
        }
    }
}

/// Render a binding as an entry parameter: `name: Type` when the type's
/// display form parses back as type syntax, else the bare name.
fn annotated_param(name: &str, ty: &Type) -> String {
    let rendered = ambient_analysis::queries::format_type(ty);
    let probe = format!("fn __probe(x: {rendered}) {{ }}");
    if ambient_parser::parse_recovering(&probe).errors.is_empty() {
        format!("{name}: {rendered}")
    } else {
        name.to_string()
    }
}
