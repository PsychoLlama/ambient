//! Tasks that wait for a cell to change. See `spec/cells.md`.
//!
//! The tasks waiting on one cell are a ring: the cell names the one
//! that has waited longest, and each names the one before it and the
//! one after. So a change to a cell costs what waits on it and nothing
//! for any other task, and a task leaves its wait at once.

use super::{Scheduler, scheduler};
use crate::cell::waited;
use crate::{Context, Word};

/// A task's wait on a cell: its place among the tasks that wait on it.
pub(super) struct Watch {
    /// The word of the cell that names the first of them
    /// ([`waited`]): a cell stays where it is, and a task that waits
    /// on one holds it.
    longest: *mut u64,
    /// The tasks around it in the ring, itself if it waits alone.
    before: usize,
    after: usize,
}

impl Scheduler {
    /// Makes `after` the task after `before` in the ring both are in.
    fn link(&mut self, before: usize, after: usize) {
        if let Some(watch) = &mut self.task(before).cell {
            watch.after = after;
        }
        if let Some(watch) = &mut self.task(after).cell {
            watch.before = before;
        }
    }

    /// Makes the task `id` wait on the cell whose word is `longest`,
    /// after those that already do.
    unsafe fn watches(&mut self, id: usize, longest: *mut u64) {
        // With none waiting it is the first, and the last before it.
        let first = unsafe { *longest }.checked_sub(1);
        let first = first.map_or(id, |first| first as usize);
        let last = self
            .task(first)
            .cell
            .as_ref()
            .map_or(id, |watch| watch.before);
        if first == id {
            unsafe { *longest = id as u64 + 1 };
        }
        self.task(id).cell = Some(Watch {
            longest,
            before: last,
            after: first,
        });
        self.link(last, id);
        self.link(id, first);
    }

    /// Takes the task `id` out of its wait on a cell, if it is in one.
    pub(super) unsafe fn unwatch(&mut self, id: usize) {
        let Some(watch) = self.task(id).cell.take() else {
            return;
        };
        let longest = unsafe { &mut *watch.longest };
        if watch.after == id {
            *longest = 0;
            return;
        }
        if *longest == id as u64 + 1 {
            *longest = watch.after as u64 + 1;
        }
        self.link(watch.before, watch.after);
    }

    /// Gives every task waiting on the cell whose word is `longest` its
    /// turn, the one that has waited longest first.
    unsafe fn wake(&mut self, longest: *mut u64) {
        let Some(first) = core::mem::take(unsafe { &mut *longest }).checked_sub(1) else {
            return;
        };
        let first = first as usize;
        let mut id = first;
        loop {
            let watch = self.task(id).cell.take();
            self.queue(id);
            match watch {
                Some(watch) if watch.after != first => id = watch.after,
                _ => break,
            }
        }
    }
}

/// Makes the current task wait on `cell`, or with none stop waiting.
pub(crate) unsafe fn wait_on(context: *mut Context, cell: Option<Word>) {
    unsafe {
        let scheduler = scheduler(context);
        let id = scheduler.current;
        scheduler.unwatch(id);
        if let Some(cell) = cell {
            scheduler.watches(id, waited(cell));
        }
    }
}

/// Gives every task waiting on `cell` its turn.
pub(crate) unsafe fn wake(context: *mut Context, cell: Word) {
    unsafe { scheduler(context).wake(waited(cell)) }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::super::{Scheduler, Task};

    /// A scheduler in which three tasks wait on the cell whose word is
    /// `longest`, and the tasks, longest waiting first.
    fn waiting(longest: &mut u64) -> (Scheduler, [usize; 3]) {
        let mut scheduler = Scheduler::new();
        let tasks = [(); 3].map(|()| scheduler.admit(Task::default()));
        for task in tasks {
            unsafe { scheduler.watches(task, longest) };
        }
        (scheduler, tasks)
    }

    /// The tasks given a turn by a change to the cell, in order.
    fn woken(scheduler: &mut Scheduler, longest: &mut u64) -> Vec<usize> {
        unsafe { scheduler.wake(longest) };
        scheduler.ready.drain(..).collect()
    }

    #[test]
    fn a_change_wakes_the_task_that_waited_longest_first() {
        let mut longest = 0;
        let (mut scheduler, tasks) = waiting(&mut longest);

        assert_eq!(woken(&mut scheduler, &mut longest), tasks);
        assert_eq!(woken(&mut scheduler, &mut longest), []);
    }

    #[test]
    fn a_task_that_leaves_its_wait_is_not_woken() {
        for leaves in 0..3 {
            let mut longest = 0;
            let (mut scheduler, mut tasks) = waiting(&mut longest);
            unsafe { scheduler.unwatch(tasks[leaves]) };
            tasks[leaves..].rotate_left(1);

            assert_eq!(woken(&mut scheduler, &mut longest), tasks[..2]);
        }
    }

    #[test]
    fn the_last_task_to_leave_a_wait_leaves_the_cell_waited_on_by_none() {
        let mut longest = 0;
        let (mut scheduler, tasks) = waiting(&mut longest);
        for task in tasks {
            unsafe { scheduler.unwatch(task) };
        }

        assert_eq!(longest, 0);
    }
}
