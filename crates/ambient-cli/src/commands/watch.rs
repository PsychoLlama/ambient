//! Shared source-watching for the live-reload frontends.
//!
//! `ambient dev` redeploys on every change; the REPL marks its session
//! stale and reloads at the next prompt interaction. Both watch the same
//! thing — `.ab` files under a package root, ignoring the package-local
//! store — so the event filter and the watcher setup live here.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

/// Whether a watch event touches an `.ab` source file, ignoring the
/// package-local store under `.ambient/` (a deploy persists hundreds of
/// objects there; reacting to those would loop forever).
pub fn is_ab_change(event: &notify::Event) -> bool {
    event.paths.iter().any(|p| {
        p.extension().is_some_and(|ext| ext == "ab")
            && !p.components().any(|c| c.as_os_str() == ".ambient")
    })
}

/// A background watcher that flags source changes under one root.
///
/// The flag is *level-triggered*: any number of change events between two
/// [`take_dirty`](Self::take_dirty) calls coalesce into one `true`, so a
/// consumer that reloads lazily (the REPL, at the next prompt interaction)
/// needs no debounce. `on_dirty` fires only on the clean→dirty transition
/// — the REPL uses it to print one "sources changed" note, not one per
/// editor save-storm event.
pub struct SourceWatcher {
    /// Kept alive for the watcher thread's lifetime.
    _watcher: RecommendedWatcher,
    dirty: Arc<AtomicBool>,
}

impl SourceWatcher {
    /// Watch `.ab` changes under `root` recursively. `on_dirty` runs on
    /// the watcher's thread at each clean→dirty transition.
    pub fn spawn(root: &Path, on_dirty: impl Fn() + Send + 'static) -> Result<Self> {
        let dirty = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&dirty);
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && is_ab_change(&event)
                    && !flag.swap(true, Ordering::SeqCst)
                {
                    on_dirty();
                }
            },
            Config::default().with_poll_interval(Duration::from_millis(200)),
        )
        .context("failed to create file watcher")?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", root.display()))?;
        Ok(Self {
            _watcher: watcher,
            dirty,
        })
    }

    /// Consume the dirty flag: `true` if any source changed since the last
    /// call (re-arming the transition notification).
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }
}
