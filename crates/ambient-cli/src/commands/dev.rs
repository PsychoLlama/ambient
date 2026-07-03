//! Dev command implementation with hot reload.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use ambient_engine::format::format_value;
use ambient_engine::vm::Vm;

use super::compile_source;

/// Run an Ambient program with hot reload on file changes.
pub fn cmd_dev(file: &Path, entry: &str, watch_dirs: Option<&[PathBuf]>) -> Result<()> {
    use std::sync::mpsc::channel;

    let file = file.canonicalize().context("failed to resolve file path")?;

    // Determine watch directories.
    let watch_paths: Vec<PathBuf> = match watch_dirs {
        Some(dirs) => dirs.to_vec(),
        None => {
            // Default to the file's parent directory.
            vec![file.parent().unwrap_or(Path::new(".")).to_path_buf()]
        }
    };

    eprintln!("\x1b[1;36m[dev]\x1b[0m Watching for changes...");
    for path in &watch_paths {
        eprintln!("      {}", path.display());
    }
    eprintln!();

    // Create a channel to receive file events.
    let (tx, rx) = channel();

    // Create a watcher.
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_millis(200)),
    )
    .context("failed to create file watcher")?;

    // Start watching directories.
    for path in &watch_paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", path.display()))?;
    }

    // Initial run.
    run_dev_iteration(&file, entry);

    // Watch for changes.
    let mut last_run = Instant::now();
    let debounce = Duration::from_millis(100);

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                // Filter to only .ab file changes.
                let is_ab_change = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "ab"));

                if is_ab_change && last_run.elapsed() > debounce {
                    last_run = Instant::now();
                    eprintln!();
                    eprintln!("\x1b[1;36m[dev]\x1b[0m File changed, reloading...");
                    eprintln!();
                    run_dev_iteration(&file, entry);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Continue waiting.
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("file watcher disconnected");
            }
        }
    }
}

/// Run a single iteration of the dev server (compile and execute).
fn run_dev_iteration(file: &Path, entry: &str) {
    let start = Instant::now();

    // Read source.
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "\x1b[1;31merror\x1b[0m: failed to read {}: {}",
                file.display(),
                e
            );
            return;
        }
    };

    // Compile.
    let compiled = match compile_source(&source, file) {
        Ok(c) => c,
        Err(_) => {
            // Error already printed by compile_source.
            return;
        }
    };

    let compile_time = start.elapsed();

    // Create and configure VM with the platform prelude's default abilities.
    let prelude = match super::platform_prelude() {
        Ok(prelude) => prelude,
        Err(e) => {
            eprintln!("\x1b[1;31merror\x1b[0m: {e}");
            return;
        }
    };
    let mut vm = Vm::new();
    ambient_platform::register_defaults(&mut vm, &prelude);

    // Load all functions into the VM.
    for func in compiled.functions.values() {
        vm.load_function(func.clone());
    }

    // Find entry point.
    let entry_hash = match compiled.function_names.get(entry) {
        Some(h) => h,
        None => {
            eprintln!("\x1b[1;31merror\x1b[0m: entry function `{entry}` not found");
            return;
        }
    };

    // Execute with stack trace support.
    let run_start = Instant::now();
    match vm.call_with_trace(entry_hash, Vec::new()) {
        Ok(result) => {
            let run_time = run_start.elapsed();
            let formatted = format_value(&result);
            eprintln!();
            eprintln!(
                "\x1b[1;32m[done]\x1b[0m {} (compile: {:?}, run: {:?})",
                formatted, compile_time, run_time
            );
        }
        Err(runtime_error) => {
            eprintln!();
            eprintln!("\x1b[1;31m{runtime_error}\x1b[0m");
        }
    }
}
