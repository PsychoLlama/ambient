//! Dev command: live-upgrade development.
//!
//! `ambient dev <pkg>` runs a program under a runtime host and redeploys
//! it on every source change. A deploy re-runs the entry function as a
//! reconciliation pass over the running system (see
//! `ref/live-upgrade.md`): the name table swaps atomically, so late
//! bindings — a task's next pass, a `Live::latest!` read — pick up
//! changed code; cells keep their values; tasks the entry stops
//! declaring are drained. For a program that ensures no tasks, a deploy
//! simply re-runs it — the classic rerun-on-change dev loop falls out
//! as the trivial case.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use ambient_engine::format::format_value;
use ambient_engine::value::Value;
use ambient_platform::task::{TaskEvent, TaskEventSink};

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

    // One host for the whole session: the task registry and cell table
    // survive redeploys — that's the point. `dev` passes no program
    // args: `Env::args!()` has no coherent meaning across the
    // reconciliation re-deploys that define the dev loop.
    let host = RuntimeHost::new(
        task_event_printer(),
        ambient_platform::StdioSink::inherit(),
        Vec::new(),
    )?;

    // Initial deploy. Failures (including compile errors) leave the dev
    // loop watching, same as any later iteration.
    deploy_iteration(&host, &path, entry);

    // Watch for changes. A change is *deferred* through the debounce —
    // never dropped: an edit is redeployed no matter when it lands
    // relative to the previous deploy. (An earlier version compared
    // against the last deploy's timestamp and discarded events inside
    // the window; an edit saved within 100ms of a deploy — easy for a
    // test, possible for an editor save storm — was silently never
    // deployed, and the watcher never refires for it.)
    let debounce = Duration::from_millis(100);

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => {
                if !is_ab_change(&event) {
                    continue;
                }
                // Coalesce the burst: wait until the tree has been quiet
                // for one debounce window (editor save storms write many
                // events; one deploy covers them all).
                loop {
                    match rx.recv_timeout(debounce) {
                        Ok(_) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            bail!("file watcher disconnected");
                        }
                    }
                }
                eprintln!();
                eprintln!("{TAG} Change detected, deploying...");
                deploy_iteration(&host, &path, entry);
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

/// Whether a watch event touches an `.ab` source file, ignoring the
/// package-local store under `.ambient/` (a deploy persists hundreds of
/// objects there; redeploying on those would loop forever).
fn is_ab_change(event: &notify::Event) -> bool {
    event.paths.iter().any(|p| {
        p.extension().is_some_and(|ext| ext == "ab")
            && !p.components().any(|c| c.as_os_str() == ".ambient")
    })
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
            let report = &outcome.report;
            let tasks = &outcome.tasks;
            let mut parts = Vec::new();
            if !report.names.rebound.is_empty() {
                parts.push(format!("rebound {}", report.names.rebound.join(", ")));
            }
            if !tasks.started.is_empty() {
                parts.push(format!("tasks started: {}", tasks.started.join(", ")));
            }
            if !tasks.drained.is_empty() {
                parts.push(format!("tasks draining: {}", tasks.drained.join(", ")));
            }
            if tasks.unchanged > 0 {
                parts.push(format!("{} task(s) unchanged", tasks.unchanged));
            }
            if !report.names.retired.is_empty() {
                // Signature-changed names never rebind: `Live::latest!`
                // keeps resolving old refs to themselves until their
                // callers upgrade (ref/live-upgrade.md, rebinding rule).
                parts.push(format!(
                    "signature changed, retired (not rebound): {}",
                    report.names.retired.join(", ")
                ));
            }
            let summary = if parts.is_empty() {
                if matches!(report.value, Value::Unit) {
                    "done".to_string()
                } else {
                    format_value(&report.value)
                }
            } else {
                parts.join(", ")
            };
            eprintln!(
                "\x1b[1;32m[deployed]\x1b[0m generation {}: {} (compile: {:?}, deploy: {:?})",
                report.generation,
                summary,
                compile_time,
                deploy_start.elapsed()
            );
            for warning in &report.warnings {
                eprintln!("\x1b[1;33m[warn]\x1b[0m {warning}");
            }
            report_retirement(&outcome.retirement);
            gc_package_store(path, &outcome.retirement);
        }
        Err(e) => {
            eprintln!();
            eprintln!("\x1b[1;31m{e}\x1b[0m");
            eprintln!("{TAG} Keeping the previous build running.");
        }
    }
}

/// Narrate the retirement trace: newly retired generations, and old
/// generations still pinned (with the value that refuses to migrate —
/// see `ref/live-upgrade.md`, "Retirement").
fn report_retirement(report: &ambient_platform::retire::RetirementReport) {
    for id in &report.newly_retired {
        eprintln!("{TAG} generation {id} retired");
    }
    for generation in &report.pinned {
        // One line per pinned generation; its first pin is the most
        // direct holder (BFS provenance), which is the diagnosis.
        if let Some(pin) = generation.pins.first() {
            eprintln!(
                "{TAG} generation {} pinned by {} ({})",
                generation.id,
                pin.root,
                pin.describe()
            );
        }
    }
}

/// Purge retired generations' objects from a package's on-disk store,
/// keeping everything the running system can still reach (the trace's
/// reachable set as extra gc roots, on top of the names index). Bare
/// files and packs have no package store — nothing to do. Failures are
/// warnings: the store is a rebuildable cache.
fn gc_package_store(path: &Path, report: &ambient_platform::retire::RetirementReport) {
    let store_path = path.join(".ambient").join("store");
    if !store_path.is_dir() {
        return;
    }
    match ambient_engine::disk_store::DiskStore::open(&store_path) {
        Ok(store) => match store.gc(&report.reachable) {
            Ok(0) => {}
            Ok(removed) => eprintln!("{TAG} store gc: removed {removed} unreachable object(s)"),
            Err(e) => eprintln!("{TAG} store gc failed: {e}"),
        },
        Err(e) => eprintln!("{TAG} store gc could not open the store: {e}"),
    }
}

/// Event sink that narrates task lifecycle to stderr.
fn task_event_printer() -> TaskEventSink {
    Arc::new(|event: &TaskEvent| match event {
        TaskEvent::Started { name } => {
            eprintln!("{TAG} task `{name}` started");
        }
        TaskEvent::Draining { name } => {
            eprintln!("{TAG} task `{name}` draining (unwinds at its next interruptible perform)");
        }
        TaskEvent::Drained { name, cleanly } => {
            if *cleanly {
                eprintln!("{TAG} task `{name}` drained");
            } else {
                eprintln!("{TAG} task `{name}` drained (no Drain::requested arm ran)");
            }
        }
        TaskEvent::Faulted {
            name,
            error,
            restarting,
        } => {
            eprintln!("\x1b[1;31m[crash]\x1b[0m task `{name}`: {error}");
            if *restarting {
                eprintln!("{TAG} task `{name}` restarting (the next pass re-resolves)");
            } else {
                eprintln!("{TAG} task `{name}` parked");
            }
        }
    })
}
