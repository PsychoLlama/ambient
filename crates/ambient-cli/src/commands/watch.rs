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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use super::*;

    /// A `.ab` write under the watched root sets the dirty flag and fires
    /// the transition callback exactly once until the flag is consumed;
    /// store writes under `.ambient/` never do.
    #[test]
    fn ab_writes_set_dirty_once_and_store_writes_do_not() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".ambient/store")).unwrap();

        let fired = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&fired);
        let watcher = SourceWatcher::spawn(dir.path(), move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .expect("spawn watcher");

        // Store writes are filtered out entirely.
        std::fs::write(dir.path().join(".ambient/store/aabbcc.ab"), b"x").unwrap();
        // Two source writes coalesce into one dirty period.
        std::fs::write(dir.path().join("one.ab"), b"pub fn a(): Number { 1 }").unwrap();
        std::fs::write(dir.path().join("two.ab"), b"pub fn b(): Number { 2 }").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        while !watcher.take_dirty() {
            assert!(Instant::now() < deadline, "watcher never flagged the .ab write");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "one dirty period must announce exactly once"
        );

        // Consumed: stays clean until the next change re-arms it.
        assert!(!watcher.take_dirty());
        std::fs::write(dir.path().join("one.ab"), b"pub fn a(): Number { 3 }").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watcher.take_dirty() {
            assert!(Instant::now() < deadline, "watcher missed the re-arm write");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fired.load(Ordering::SeqCst), 2);
    }
}
