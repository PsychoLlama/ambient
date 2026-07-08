//! Run command implementation.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use ambient_engine::build::BuildOptions;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::format::format_value_colored;
use ambient_platform::process::ProcessEvent;

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
    let prelude = super::platform_prelude()?;
    let result = ambient_engine::build::build_package(
        path,
        super::parse_source,
        &BuildOptions {
            platform_source: ambient_platform::ABILITY_DECLARATIONS,
            prelude_abilities: &prelude,
            natives: None,
            progress: None,
        },
    )
    .map_err(report_build_error)?;

    // Persist the build to the package-local content-addressed store.
    // Failure to persist is a warning, not a failed run.
    match ambient_engine::disk_store::DiskStore::open(path.join(".ambient").join("store")) {
        Ok(disk) => {
            if let Err(e) = disk.put_module(&result.compiled) {
                eprintln!("warning: failed to persist build to store: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to open package store: {e}"),
    }

    Ok(result.compiled)
}

/// Run a compiled module.
///
/// The entry runs as the initial deploy pass of a process runtime. A
/// program that spawns no processes behaves exactly as before: the
/// entry runs to completion and the command exits. A program that
/// spawns processes keeps running until every process has exited.
fn run_compiled(compiled: &CompiledModule, entry: &str, program_args: Vec<String>) -> Result<()> {
    // `run` is quiet about routine lifecycle; only failures print.
    let events = Arc::new(|event: &ProcessEvent| match event {
        ProcessEvent::Crashed {
            name,
            error,
            restarting,
        } => {
            eprintln!("process `{name}` crashed: {error}");
            if *restarting {
                eprintln!("process `{name}` restarting with fresh state");
            } else {
                eprintln!("process `{name}` exceeded its fault budget; giving up");
            }
        }
        ProcessEvent::InitFailed { name, error } => {
            eprintln!("process `{name}` failed to initialize: {error}");
        }
        _ => {}
    });

    let host = RuntimeHost::new(events, program_args)?;

    match host.deploy(compiled, entry) {
        Ok(outcome) => {
            // Print result if not unit.
            if !matches!(outcome.value, ambient_engine::value::Value::Unit) {
                println!("{}", format_value_colored(&outcome.value));
            }
        }
        Err(runtime_error) => {
            // Print rich error with stack trace.
            eprintln!("{runtime_error}");
            bail!("runtime error");
        }
    }

    // Block until the process tree (if any) winds down.
    host.runtime().wait_all();
    Ok(())
}
