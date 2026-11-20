//! Answers a native bundle's own threads bring. See "Native bundles"
//! in `docs/spec/externs.md`.
//!
//! A thread of a bundle holds no value: the runtime is one thread's.
//! So it posts what makes the answer, and the program's thread makes
//! it when no task can run without one. A post names its wait by a
//! number no other program of the process has, since a thread is
//! handed nothing else to say which program it answers.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};

use ambient_native::{Data, Make};

/// What a thread posted for a wait.
pub(super) enum Post {
    /// What makes the answer, and what it is handed: an address, kept
    /// as a number since only the bundle reads it.
    Made(Make, usize),
    Refused(String),
}

#[derive(Default)]
struct State {
    /// What was posted, each for the ticket of its wait, or for none
    /// if its task has left.
    posted: VecDeque<(Option<u64>, Post)>,
    /// The ticket of each wait a post may still come for, by the
    /// number a bundle knows it by.
    waiting: BTreeMap<u64, u64>,
    /// What to call when something is posted.
    wake: Option<Box<dyn Fn() + Send>>,
}

/// The posts of one program.
#[derive(Default)]
pub struct Posts {
    state: Mutex<State>,
    came: Condvar,
}

/// Each program's posts, by the numbers its waits are known by.
static NAMED: Mutex<BTreeMap<u64, Weak<Posts>>> = Mutex::new(BTreeMap::new());

/// What a lock holds, whether or not a thread panicked holding it:
/// nothing here is left half done.
fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Posts {
    /// Has `wake` called, on whatever thread posts, each time
    /// something is posted: how a host that waits elsewhere is told to
    /// look here.
    pub fn wake_with(&self, wake: Box<dyn Fn() + Send>) {
        held(&self.state).wake = Some(wake);
    }

    /// Whether something was posted that the program has not taken.
    #[must_use]
    pub fn ready(&self) -> bool {
        !held(&self.state).posted.is_empty()
    }

    /// Whether a post may still come.
    #[must_use]
    pub fn coming(&self) -> bool {
        !held(&self.state).waiting.is_empty()
    }

    /// The number a bundle is to answer the wait of `ticket` by.
    pub(super) fn name(self: &Arc<Self>, ticket: u64) -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let name = NEXT.fetch_add(1, Ordering::Relaxed);
        held(&self.state).waiting.insert(name, ticket);
        held(&NAMED).insert(name, Arc::downgrade(self));
        name
    }

    /// Says the task that held `ticket` left its wait: no post is
    /// taken for it from here on, and one already made answers nothing.
    pub(super) fn forsake(&self, ticket: u64) {
        let mut state = held(&self.state);
        let left: Vec<u64> = state
            .waiting
            .iter()
            .filter(|(_, held)| **held == ticket)
            .map(|(name, _)| *name)
            .collect();
        for name in left {
            state.waiting.remove(&name);
            held(&NAMED).remove(&name);
        }
        for (answers, _) in &mut state.posted {
            *answers = answers.filter(|answers| *answers != ticket);
        }
    }

    /// The next post, and the ticket it answers, if its task waits
    /// still.
    pub(super) fn take(&self) -> Option<(Option<u64>, Post)> {
        held(&self.state).posted.pop_front()
    }

    /// Waits until something is posted.
    pub(super) fn wait(&self) {
        let mut state = held(&self.state);
        while state.posted.is_empty() {
            state = self
                .came
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Drop for Posts {
    fn drop(&mut self) {
        let state = held(&self.state);
        let mut named = held(&NAMED);
        for name in state.waiting.keys() {
            named.remove(name);
        }
    }
}

/// Posts `post` for the wait a bundle knows as `name`, from any
/// thread. False if nothing waits by that name.
fn post(name: u64, post: Post) -> bool {
    let Some(posts) = held(&NAMED).remove(&name).and_then(|posts| posts.upgrade()) else {
        return false;
    };
    let mut state = held(&posts.state);
    let Some(ticket) = state.waiting.remove(&name) else {
        return false;
    };
    state.posted.push_back((Some(ticket), post));
    if let Some(wake) = &state.wake {
        wake();
    }
    posts.came.notify_all();
    true
}

/// Answers the wait of a ticket [`super::amb_bundle_later`] gave, from
/// any thread. `make` is called once with `data` on the program's
/// thread, and what it gives is the answer.
///
/// Zero, with `make` never called, if the task has left the wait.
///
/// # Safety
///
/// `make` may be called with `data` on another thread, later.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_answer(ticket: u64, make: Make, data: *mut Data) -> u64 {
    u64::from(post(ticket, Post::Made(make, data as usize)))
}

/// Fails the wait of a ticket [`super::amb_bundle_later`] gave, from
/// any thread, with a message of `length` bytes. Zero if the task has
/// left the wait.
///
/// # Safety
///
/// `message` is `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_bundle_refuse(ticket: u64, message: *const u8, length: u64) -> u64 {
    let length = usize::try_from(length).unwrap_or(0);
    // SAFETY: the caller's to uphold.
    let message = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, length) });
    u64::from(post(ticket, Post::Refused(message.into_owned())))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ambient_runtime::Word;

    use super::{Data, Post, Posts, post};

    unsafe extern "C" fn made(_: *mut Data) -> Word {
        ambient_runtime::NULL
    }

    fn posted(name: u64) -> bool {
        post(name, Post::Made(made, 0))
    }

    #[test]
    fn a_post_is_taken_for_the_ticket_its_wait_holds() {
        let posts = Arc::new(Posts::default());
        let name = posts.name(7);

        assert!(posts.coming() && !posts.ready());
        assert!(posted(name));
        assert!(!posts.coming());
        assert!(matches!(posts.take(), Some((Some(7), _))));
    }

    #[test]
    fn a_wait_is_answered_once() {
        let posts = Arc::new(Posts::default());
        let name = posts.name(7);

        assert!(posted(name));
        assert!(!posted(name));
    }

    #[test]
    fn a_post_for_a_wait_its_task_left_answers_nothing() {
        let posts = Arc::new(Posts::default());
        let (before, after) = (posts.name(7), posts.name(8));
        assert!(posted(before));
        posts.forsake(7);
        posts.forsake(8);

        assert!(!posted(after));
        assert!(matches!(posts.take(), Some((None, _))));
    }

    #[test]
    fn a_post_for_a_program_that_is_over_is_refused() {
        let posts = Arc::new(Posts::default());
        let name = posts.name(7);
        drop(posts);

        assert!(!posted(name));
    }
}
