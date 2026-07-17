//! Run command implementation.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use ambient_engine::build::BuildOptions;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_platform::task::TaskEvent;

use super::host::RuntimeHost;
use crate::diagnostic::report_build_error;

/// Run an Ambient package or pre-compiled artifact.
///
/// If `path` is a directory (or contains an `ambient.toml`), runs the package.
/// If `path` is a `.ambient` file, runs the pre-compiled artifact pack.
///
/// `args` are the trailing program arguments (everything after `--`).
/// They become `core::system::Env::args!()` with the program path — the
/// `path` argument as typed — at index 0, mirroring Python's `sys.argv[0]`
/// / Go's `os.Args[0]`.
pub fn cmd_run(path: &Path, entry: &str, args: Vec<String>, package: Option<&str>) -> Result<()> {
    let program_args = std::iter::once(path.to_string_lossy().into_owned())
        .chain(args)
        .collect::<Vec<_>>();
    let (compiled, entry) = load_compiled(path, Some((entry, package)))?;
    run_compiled(&compiled, &entry, program_args)
}

/// Load a compiled module from a path, returning it with the entry name to
/// deploy (canonically qualified to the target package, so a workspace
/// dependency's same-named function can never be picked instead).
///
/// Handles packages (directories with `ambient.toml`), pre-compiled
/// `.ambient` artifact packs, and bare `.ab` source files.
///
/// `entry` selects the build strategy for a package directory:
/// `Some((name, package))` (`ambient run`) builds **lazily** — only the
/// modules reachable from that entry in the target package, reading but not
/// writing the store. `None` (`ambient dev`) builds the whole target and
/// persists a snapshot, since a deploy needs every module's bindings and
/// the snapshot writers keep the store complete. It is ignored for
/// `.ab`/`.ambient` inputs, which never lazily build a package.
pub(super) fn load_compiled(
    path: &Path,
    entry: Option<(&str, Option<&str>)>,
) -> Result<(CompiledModule, String)> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let spelled_entry = entry.map_or("run", |(name, _)| name);

    if ext == "ab" && path.is_file() {
        // Compile a bare source file against the core library.
        let source = super::read_source(path)?;
        return Ok((
            super::compile_source(&source, path)?,
            spelled_entry.to_string(),
        ));
    }

    if ext == "ambient" {
        // Load a pre-compiled artifact pack. Function hashes are recomputed
        // from the object bytes, so a tampered artifact fails to load.
        let bytes = fs::read(path).context("failed to read file")?;
        let pack = ambient_engine::store::Pack::decode(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))?;
        let compiled = CompiledModule::from_pack(&pack)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))?;
        Ok((compiled, spelled_entry.to_string()))
    } else if path.is_dir() || path.join("ambient.toml").exists() {
        // Load package: lazily for `ambient run` (an entry is given), or
        // whole-package + persist for `ambient dev`.
        match entry {
            Some((entry, package)) => compile_package(path, entry, package),
            None => {
                let target = super::resolve_target_package(path, None)?;
                let compiled = compile_package_full(path, target.as_deref())?;
                Ok((compiled, super::qualified_entry(target.as_deref(), "run")))
            }
        }
    } else {
        bail!(
            "expected a directory with ambient.toml or a .ambient file, got: {}",
            path.display()
        );
    }
}

/// Compile a package from its root directory for `ambient run`.
///
/// This is a **lazy** build ([`ambient_engine::build::build_reachable`]): it
/// compiles only the modules reachable from the `entry` point, reading the
/// package-local store for warm cache hits but writing no snapshot. Whole-package
/// snapshot writing (and thus cache warming) is `ambient build`/`ambient dev`'s
/// job — a lazy run must not persist a partial build (see `ref/modules.md`).
/// Type errors in modules the entry can't reach are, by policy, not reported by
/// `run`; `ambient check` reports them.
pub(super) fn compile_package(
    path: &Path,
    entry: &str,
    package: Option<&str>,
) -> Result<(CompiledModule, String)> {
    // Stub natives satisfy the extern contract at build time (real
    // implementations are registered per VM by the runtime host).
    let stubs = ambient_platform::stub_natives();
    let target = super::resolve_target_package(path, package)?;
    let entry = super::qualified_entry(target.as_deref(), entry);
    let result = ambient_engine::build::build_reachable(
        path,
        super::parse_source,
        BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            package: target.as_deref(),
            ..Default::default()
        },
        &entry,
    )
    .map_err(report_build_error)?;

    Ok((result.compiled, entry))
}

/// Compile a package whole (every module) and persist a snapshot, for
/// `ambient dev`.
///
/// Delegates to the engine's shared build-and-persist wiring
/// ([`ambient_engine::build::build_and_persist`]), which reads the prior
/// snapshot and persists the new build to the package-local content-addressed
/// store. Persist failure is a warning, not a failed run — the store is a
/// rebuildable cache. Unlike `ambient run`, `dev` stays whole-package: its
/// deploy diff needs every module's bindings, and it is the command whose
/// snapshot keeps the store complete for later lazy runs.
pub(super) fn compile_package_full(path: &Path, package: Option<&str>) -> Result<CompiledModule> {
    // Stub natives satisfy the extern contract at build time (real
    // implementations are registered per VM by the runtime host).
    let stubs = ambient_platform::stub_natives();
    let built = ambient_engine::build::build_and_persist(
        path,
        super::parse_source,
        BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            package,
            ..Default::default()
        },
    )
    .map_err(report_build_error)?;

    if let Err(e) = built.persisted {
        eprintln!("warning: failed to persist build to store: {e}");
    }

    Ok(built.result.compiled)
}

/// Run a compiled module.
///
/// The entry runs as the initial deploy pass over an empty task
/// registry. A program that ensures no tasks behaves exactly as before:
/// the entry runs to completion and the command exits. Otherwise the
/// command keeps running until every task has wound down.
fn run_compiled(compiled: &CompiledModule, entry: &str, program_args: Vec<String>) -> Result<()> {
    // `run` is quiet about routine lifecycle; only failures print.
    let task_events = Arc::new(|event: &TaskEvent| {
        if let TaskEvent::Faulted {
            name,
            error,
            restarting,
        } = event
        {
            eprintln!("task `{name}` faulted: {error}");
            if *restarting {
                eprintln!("task `{name}` restarting");
            } else {
                eprintln!("task `{name}` parked");
            }
        }
    });

    let host = RuntimeHost::new(
        task_events,
        ambient_platform::StdioSink::inherit(),
        program_args,
    )?;

    match host.deploy(compiled, entry) {
        Ok(outcome) => {
            // Print result if not unit.
            if !matches!(outcome.report.value, ambient_engine::value::Value::Unit) {
                println!("{}", format_value_colored(&outcome.report.value));
            }
        }
        Err(runtime_error) => {
            // Print rich error with stack trace.
            eprintln!("{runtime_error}");
            bail!("runtime error");
        }
    }

    // Block until the task registry (if any) winds down. Under plain
    // `run` nothing ever drains a task, so an ensured task keeps the
    // program alive — the acceptor-loop shape.
    host.tasks().wait_all();
    Ok(())
}
