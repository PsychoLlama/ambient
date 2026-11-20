//! What a thread brings a program that waits for it, held in order
//! and bounded. See "Calls from the world" in `docs/spec/targets.md`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most of anything a thread brings that is held while nothing
/// has taken it: connections, datagrams, changes.
pub(super) const HELD: usize = 256;

/// What waits on something a thread brings, and what it brought that
/// nothing has taken.
pub(super) struct Queue<T> {
    untaken: VecDeque<T>,
    pub(super) waiting: VecDeque<u64>,
    /// How many its thread has brought that nothing has taken: the
    /// thread drops one more than [`HELD`].
    held: Arc<AtomicUsize>,
    /// Marks it closed and lets its thread go.
    stop: Box<dyn Fn() + Send>,
}

impl<T> Queue<T> {
    pub(super) fn new(held: Arc<AtomicUsize>, stop: Box<dyn Fn() + Send>) -> Self {
        Self {
            untaken: VecDeque::new(),
            waiting: VecDeque::new(),
            held,
            stop,
        }
    }

    /// The next that was brought, or the ticket waits for one.
    pub(super) fn take(&mut self, ticket: u64) -> Option<T> {
        let next = self.untaken.pop_front();
        if next.is_some() {
            self.held.fetch_sub(1, Ordering::Relaxed);
        } else {
            self.waiting.push_back(ticket);
        }
        next
    }

    /// Hands what was brought to a ticket that waits, or keeps it.
    pub(super) fn bring(&mut self, brought: T) -> Option<(u64, T)> {
        let Some(ticket) = self.waiting.pop_front() else {
            self.untaken.push_back(brought);
            return None;
        };
        self.held.fetch_sub(1, Ordering::Relaxed);
        Some((ticket, brought))
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        (self.stop)();
    }
}

/// Whether a thread may bring one more, which is then counted.
pub(super) fn room(held: &AtomicUsize) -> bool {
    let has = held.load(Ordering::Relaxed) < HELD;
    if has {
        held.fetch_add(1, Ordering::Relaxed);
    }
    has
}
