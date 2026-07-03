//! Dev command: live-upgrade development.
//!
//! `ambient dev <pkg>` runs a program under the process runtime and
//! redeploys it on every source change. A deploy re-runs the entry
//! function as a reconciliation pass over the live process tree (see
//! `ref/processes.md`): processes whose reducer's content hash changed
//! swap code at their next message and **keep their state**; unchanged
//! processes are untouched; removed ones stop; new ones start. For a
//! program that spawns no processes, a deploy simply re-runs it — the
//! classic rerun-on-change dev loop falls out as the trivial case.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use ambient_engine::format::format_value;
use ambient_engine::value::Value;
use ambient_platform::process::ProcessEvent;

use super::host::RuntimeHost;

/// Prefix for dev-loop status lines.
const TAG: &str = "\x1b[1;36m[dev]\x1b[0m";

/// Run an Ambient program with live upgrade on file changes.
pub fn cmd_dev(path: &Path, entry: &str, watch_dirs: Option<&[PathBuf]>) -> Result<()> {
    use std::sync::mpsc::channel;

    let path = path.canonicalize().context("failed to resolve path")?;

    // Determine watch directories: explicit dirs, a package's root, or a
    // bare file's parent.
    let watch_paths: Vec<PathBuf> = match watch_dirs {
        Some(dirs) => dirs.to_vec(),
        None if path.is_dir() => vec![path.clone()],
        None => vec![path.parent().unwrap_or(Path::new(".")).to_path_buf()],
    };

    eprintln!("{TAG} Watching for changes...");
    for watch_path in &watch_paths {
        eprintln!("      {}", watch_path.display());
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
    for watch_path in &watch_paths {
        watcher
            .watch(watch_path, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", watch_path.display()))?;
    }

    // One host for the whole session: the process tree survives
    // redeploys — that's the point.
    let host = RuntimeHost::new(event_printer())?;

    // Initial deploy. Failures (including compile errors) leave the dev
    // loop watching, same as any later iteration.
    deploy_iteration(&host, &path, entry);

    // Watch for changes.
    let mut last_run = Instant::now();
    let debounce = Duration::from_millis(100);

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                // Filter to only .ab file changes, ignoring the
                // package-local store under .ambient/.
                let is_ab_change = event.paths.iter().any(|p| {
                    p.extension().is_some_and(|ext| ext == "ab")
                        && !p.components().any(|c| c.as_os_str() == ".ambient")
                });

                if is_ab_change && last_run.elapsed() > debounce {
                    last_run = Instant::now();
                    eprintln!();
                    eprintln!("{TAG} Change detected, deploying...");
                    deploy_iteration(&host, &path, entry);
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

/// Compile and deploy one generation. Errors are reported and leave the
/// currently running generation untouched.
fn deploy_iteration(host: &RuntimeHost, path: &Path, entry: &str) {
    let start = Instant::now();

    let compiled = match super::run::load_compiled(path) {
        Ok(compiled) => compiled,
        Err(e) => {
            // Diagnostics were already printed for parse/type errors.
            eprintln!("\x1b[1;31merror\x1b[0m: {e}");
            eprintln!("{TAG} Keeping the previous build running.");
            return;
        }
    };
    let compile_time = start.elapsed();

    let deploy_start = Instant::now();
    match host.deploy(&compiled, entry) {
        Ok(outcome) => {
            let mut parts = Vec::new();
            if !outcome.started.is_empty() {
                parts.push(format!("started {}", outcome.started.join(", ")));
            }
            if !outcome.upgraded.is_empty() {
                parts.push(format!("upgraded {}", outcome.upgraded.join(", ")));
            }
            if !outcome.stopped.is_empty() {
                parts.push(format!("stopped {}", outcome.stopped.join(", ")));
            }
            if outcome.unchanged > 0 {
                parts.push(format!("{} unchanged", outcome.unchanged));
            }
            let summary = if parts.is_empty() {
                if matches!(outcome.value, Value::Unit) {
                    "done".to_string()
                } else {
                    format_value(&outcome.value)
                }
            } else {
                parts.join(", ")
            };
            eprintln!(
                "\x1b[1;32m[deployed]\x1b[0m {} (compile: {:?}, deploy: {:?})",
                summary,
                compile_time,
                deploy_start.elapsed()
            );
        }
        Err(e) => {
            eprintln!();
            eprintln!("\x1b[1;31m{e}\x1b[0m");
            eprintln!("{TAG} Keeping the previous build running.");
        }
    }
}

/// Event sink that narrates process lifecycle to stderr.
fn event_printer() -> Arc<dyn Fn(&ProcessEvent) + Send + Sync> {
    Arc::new(|event: &ProcessEvent| match event {
        ProcessEvent::Started { name, pid } => {
            eprintln!("{TAG} process `{name}` started (pid {pid})");
        }
        ProcessEvent::Upgraded { name } => {
            eprintln!("{TAG} process `{name}` upgraded (state kept)");
        }
        ProcessEvent::Stopped { name } => {
            eprintln!("{TAG} process `{name}` stopped (no longer declared)");
        }
        ProcessEvent::Exited { name } => {
            eprintln!("{TAG} process `{name}` exited");
        }
        ProcessEvent::Crashed {
            name,
            error,
            restarting,
        } => {
            eprintln!("\x1b[1;31m[crash]\x1b[0m process `{name}`: {error}");
            if *restarting {
                eprintln!("{TAG} process `{name}` restarting with fresh state");
            } else {
                eprintln!(
                    "{TAG} process `{name}` exceeded its fault budget; parked until the next deploy"
                );
            }
        }
        ProcessEvent::InitFailed { name, error } => {
            eprintln!("\x1b[1;31m[crash]\x1b[0m process `{name}` failed to initialize: {error}");
        }
    })
}
