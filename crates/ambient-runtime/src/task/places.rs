//! The places tasks and joins have in the scheduler. A task's number
//! is its place, and a place is the next task's once its task is over,
//! so a program that goes on starting tasks does not go on growing.
//!
//! Whatever names a task by number therefore stops naming it when it
//! is over: the join that started it.

use alloc::vec::Vec;

use super::{Gives, Join, NOBODY, Scheduler, Task};
use crate::object::amb_release;
use crate::{NULL, Word};

impl Scheduler {
    /// Whether `id` is a task that is not over.
    pub(super) fn live(&self, id: usize) -> bool {
        self.tasks.get(id).is_some_and(Option::is_some)
    }

    /// Makes `task` a task, in a place none has. Gives its number.
    pub(super) fn admit(&mut self, task: Task) -> usize {
        place(&mut self.tasks, &mut self.idle, task)
    }

    /// Makes `join` a join. Gives its number.
    pub(super) fn open(&mut self, join: Join) -> usize {
        place(&mut self.joins, &mut self.closed, join)
    }

    /// Ends the join `id`, and gives what it was.
    pub(super) fn close(&mut self, id: usize) -> Option<Join> {
        let join = self.joins.get_mut(id)?.take()?;
        self.closed.push(id);
        Some(join)
    }

    /// Records that a task is over, with `result` if it gave one.
    pub(super) unsafe fn finish(&mut self, id: usize, result: Option<Word>) {
        let Some(task) = self.tasks[id].take() else {
            return;
        };
        self.idle.push(id);
        unsafe { amb_release(task.thunk) };
        let Some((join, place)) = task.place else {
            return;
        };
        // Its number is another task's from here on, so the join that
        // started it stops naming it. A group keeps only the tasks
        // that are not over, however many it starts.
        let moved = match self.joins[join].as_mut() {
            Some(join) if join.children.get(place) != Some(&id) => return,
            Some(join) if join.gives == Gives::Nothing => {
                join.children.swap_remove(place);
                join.children.get(place).copied()
            }
            Some(join) => {
                join.children[place] = NOBODY;
                None
            }
            None => return,
        };
        if let Some(moved) = moved {
            self.task(moved).place = Some((join, place));
        }
        let Some(join) = self.joins[join].as_mut() else {
            return;
        };
        if join.gives == Gives::Nothing {
            unsafe { amb_release(result.unwrap_or(NULL)) };
        } else {
            join.results[place] = result.unwrap_or(NULL);
        }
        join.remaining -= 1;
        let parent = join.parent;
        let remaining = join.remaining;
        let won = join.gives == Gives::First
            && result.is_some()
            && join.winner.is_none()
            && join.failure.is_none();
        let losers = if won {
            join.winner = Some(place);
            join.children.clone()
        } else {
            Vec::new()
        };
        for loser in losers {
            if self.live(loser) {
                unsafe { self.cancel(loser) };
            }
        }
        if remaining == 0 {
            self.queue(parent);
        }
    }
}

/// Puts `value` in a place of `held` that `free` says holds nothing, or
/// in a new one. Gives the place.
fn place<T>(held: &mut Vec<Option<T>>, free: &mut Vec<usize>, value: T) -> usize {
    if let Some(id) = free.pop() {
        held[id] = Some(value);
        return id;
    }
    held.push(Some(value));
    held.len() - 1
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::super::{Gives, Join, ROOT, Scheduler, Task};

    fn group(parent: usize) -> Join {
        Join {
            parent,
            children: Vec::new(),
            results: Vec::new(),
            remaining: 0,
            failure: None,
            gives: Gives::Nothing,
            winner: None,
            closing: false,
        }
    }

    #[test]
    fn a_task_that_is_over_leaves_its_place_to_the_next() {
        let mut scheduler = Scheduler::new();
        let first = scheduler.admit(Task::default());
        unsafe { scheduler.finish(first, None) };

        assert_eq!(scheduler.admit(Task::default()), first);
    }

    #[test]
    fn a_join_that_is_closed_leaves_its_place_to_the_next() {
        let mut scheduler = Scheduler::new();
        let first = scheduler.open(group(ROOT));
        scheduler.close(first);

        assert_eq!(scheduler.open(group(ROOT)), first);
    }

    #[test]
    fn a_group_names_only_its_tasks_that_are_not_over() {
        let mut scheduler = Scheduler::new();
        let join = scheduler.open(group(ROOT));
        let tasks = [0, 1, 2].map(|place| {
            let place = Some((join, place));
            scheduler.admit(Task {
                place,
                ..Task::default()
            })
        });
        if let Some(Some(join)) = scheduler.joins.get_mut(join) {
            join.children = tasks.to_vec();
            join.remaining = 3;
        }
        unsafe { scheduler.finish(tasks[0], None) };

        // The last took the place of the first, and knows it did.
        let children = scheduler.joins[join]
            .as_ref()
            .map(|join| join.children.clone());
        assert_eq!(children, Some(vec![tasks[2], tasks[1]]));
        assert_eq!(scheduler.task(tasks[2]).place, Some((join, 0)));
    }
}
