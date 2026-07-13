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
pub fn cmd_run(path: &Path, entry: &str, args: Vec<String>) -> Result<()> {
    let program_args = std::iter::once(path.to_string_lossy().into_owned())
        .chain(args)
        .collect::<Vec<_>>();
    let compiled = load_compiled(path)?;
    run_compiled(&compiled, entry, program_args)
}

/// Load a compiled module from a path.
///
/// Handles packages (directories with `ambient.toml`), pre-compiled
/// `.ambient` artifact packs, and bare `.ab` source files.
pub(super) fn load_compiled(path: &Path) -> Result<CompiledModule> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "ab" && path.is_file() {
        // Compile a bare source file against the core library.
        let source = super::read_source(path)?;
        return super::compile_source(&source, path);
    }

    if ext == "ambient" {
        // Load a pre-compiled artifact pack. Function hashes are recomputed
        // from the object bytes, so a tampered artifact fails to load.
        let bytes = fs::read(path).context("failed to read file")?;
        let pack = ambient_engine::store::Pack::decode(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))?;
        CompiledModule::from_pack(&pack)
            .map_err(|e| anyhow::anyhow!("invalid artifact {}: {e}", path.display()))
    } else if path.is_dir() || path.join("ambient.toml").exists() {
        // Load package.
        compile_package(path)
    } else {
        bail!(
            "expected a directory with ambient.toml or a .ambient file, got: {}",
            path.display()
        );
    }
}

/// Compile a package from its root directory.
///
/// Delegates to the engine's single package pipeline ([`ambient_engine::build::build_package`])
/// and adds the run-specific concern: persisting the build to the
/// package-local content-addressed store.
pub(super) fn compile_package(path: &Path) -> Result<CompiledModule> {
    // Stub natives satisfy the extern contract at build time (real
    // implementations are registered per VM by the runtime host).
    let stubs = ambient_platform::stub_natives();
    let result = ambient_engine::build::build_package(
        path,
        super::parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            progress: None,
        },
    )
    .map_err(report_build_error)?;

    // Persist the build to the package-local content-addressed store.
    // Failure to persist is a warning, not a failed run.
    match ambient_engine::disk_store::DiskStore::open(path.join(".ambient").join("store")) {
        Ok(disk) => {
            if let Err(e) = persist_build(&disk, &result) {
                eprintln!("warning: failed to persist build to store: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to open package store: {e}"),
    }

    Ok(result.compiled)
}

/// Persist a completed build: objects and name bindings first, then the
/// crash-safe snapshot. Ordering matters — the snapshot's root pointer is
/// only swapped after every object it references and the manifest bytes are
/// durably in place ([`DiskStore::write_snapshot`]), so a snapshot always
/// resolves to a consistent build.
fn persist_build(
    disk: &ambient_engine::disk_store::DiskStore,
    result: &ambient_engine::build::BuildResult,
) -> Result<(), ambient_engine::disk_store::DiskStoreError> {
    disk.put_module(&result.compiled)?;
    let manifest = ambient_engine::disk_store::BuildManifest::from_build(result);
    disk.write_snapshot(&manifest)?;
    Ok(())
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
