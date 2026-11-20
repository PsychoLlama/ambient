//! Watching files for a real process: the system tells a thread what
//! changed, and the thread brings it to the program. See "Standard
//! targets" in `docs/spec/targets.md`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::task::Poll;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecursiveMode, Watcher};

use super::pending::Sent;
use super::queue::{Queue, room};
use super::{Failure, Kind};

/// How something a watcher watches changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    Created,
    /// What it holds, or what is said of it, is different.
    Modified,
    Removed,
    /// Changes were not kept: more came than were taken. Whatever is
    /// watched may be different.
    Missed,
}

impl Changed {
    /// What the tag that names it is called in `system.fs`.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Removed => "removed",
            Self::Missed => "missed",
        }
    }
}

/// A change, and the path of what changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: Changed,
}

struct Watched {
    /// What was asked to be watched: where a change that was missed
    /// is said to be.
    path: String,
    changes: Queue<Change>,
    /// Whether a change came that there was no room for.
    missed: Arc<AtomicBool>,
    /// Watches for as long as it is held.
    _watcher: notify::RecommendedWatcher,
}

#[derive(Default)]
pub(super) struct Watches {
    watched: HashMap<i64, Watched>,
    numbered: i64,
}

/// The changes an event of the system is, each with its path. A path
/// that is not text is not the program's to read.
fn changes(event: notify::Event) -> Vec<Change> {
    use Changed::{Created, Modified, Removed};
    let kinds: &[Changed] = match event.kind {
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => &[Created],
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => &[Removed],
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => &[Removed, Created],
        EventKind::Modify(_) | EventKind::Any => &[Modified],
        EventKind::Access(_) | EventKind::Other => &[],
    };
    // One kind is of every path; a rename's two are of one path each.
    let each = |(at, path): (usize, PathBuf)| {
        let kind = *kinds.get(at).or(kinds.first())?;
        let path = path.into_os_string().into_string().ok()?;
        Some(Change { path, kind })
    };
    event
        .paths
        .into_iter()
        .enumerate()
        .filter_map(each)
        .collect()
}

impl Watches {
    /// Whether a thread may yet send something a task waits for.
    pub(super) fn waits(&self) -> bool {
        let waited = |watched: &Watched| !watched.changes.waiting.is_empty();
        self.watched.values().any(waited)
    }

    /// Starts watching a file, or a directory and all it holds.
    pub(super) fn watch(&mut self, path: &str, sender: Sender<Sent>) -> Result<i64, Failure> {
        let refused = |error: notify::Error| {
            let kind = match &error.kind {
                notify::ErrorKind::PathNotFound => Kind::NotFound,
                notify::ErrorKind::Io(error) => super::system::failure(error).kind,
                _ => Kind::Failed,
            };
            Failure {
                kind,
                message: error.to_string(),
            }
        };
        self.numbered += 1;
        let id = self.numbered;
        let (held, missed) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (count, lost) = (Arc::clone(&held), Arc::clone(&missed));
        let bring = move |event: notify::Result<notify::Event>| {
            for change in event.map(changes).unwrap_or_default() {
                if room(&count) {
                    let _ = sender.send(Sent::Changed(id, Some(change)));
                } else if !lost.swap(true, Ordering::Relaxed) {
                    // Wakes whatever waits, to be told one was missed.
                    let _ = sender.send(Sent::Changed(id, None));
                }
            }
        };
        let mut system = notify::recommended_watcher(bring).map_err(refused)?;
        system
            .watch(path.as_ref(), RecursiveMode::Recursive)
            .map_err(refused)?;
        let watched = Watched {
            path: path.to_owned(),
            changes: Queue::new(held, Box::new(|| {})),
            missed,
            _watcher: system,
        };
        self.watched.insert(id, watched);
        Ok(id)
    }

    /// The next change, or nothing if the watcher is closed; pending
    /// if the ticket waits for one. Once those that were kept are
    /// taken, one that was missed is told.
    pub(super) fn next(&mut self, ticket: u64, id: i64) -> Poll<Option<Change>> {
        let Some(watched) = self.watched.get_mut(&id) else {
            return Poll::Ready(None);
        };
        if let Some(change) = watched.changes.take(ticket) {
            return Poll::Ready(Some(change));
        }
        if !watched.missed.swap(false, Ordering::Relaxed) {
            return Poll::Pending;
        }
        watched.changes.waiting.retain(|waiting| *waiting != ticket);
        Poll::Ready(Some(Change {
            path: watched.path.clone(),
            kind: Changed::Missed,
        }))
    }

    /// Hands a change the system told of to a ticket that waits, or
    /// keeps it. No change is word that one was missed.
    pub(super) fn came(&mut self, id: i64, change: Option<Change>) -> Option<(u64, Change)> {
        let watched = self.watched.get_mut(&id)?;
        if let Some(change) = change {
            return watched.changes.bring(change);
        }
        let ticket = *watched.changes.waiting.front()?;
        match self.next(ticket, id) {
            Poll::Ready(Some(missed)) => Some((ticket, missed)),
            _ => None,
        }
    }

    /// Stops watching, and gives the tickets that waited for a change.
    pub(super) fn close(&mut self, id: i64) -> Vec<u64> {
        let watched = self.watched.remove(&id);
        watched.map_or_else(Vec::new, |mut watched| {
            std::mem::take(&mut watched.changes.waiting).into()
        })
    }

    pub(super) fn forsake(&mut self, ticket: u64) {
        for watched in self.watched.values_mut() {
            watched.changes.waiting.retain(|waiting| *waiting != ticket);
        }
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, EventKind, ModifyKind, RenameMode};

    use super::{Change, Changed, changes};

    fn change(path: &str, kind: Changed) -> Change {
        Change {
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn a_rename_is_one_path_removed_and_another_created() {
        let renamed = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("old".into())
            .add_path("new".into());
        let created = notify::Event::new(EventKind::Create(CreateKind::File)).add_path("a".into());

        assert_eq!(
            changes(renamed),
            [
                change("old", Changed::Removed),
                change("new", Changed::Created)
            ]
        );
        assert_eq!(changes(created), [change("a", Changed::Created)]);
    }
}
