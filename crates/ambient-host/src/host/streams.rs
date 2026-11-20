//! The streams of a real process: bytes read and written on threads
//! of their own, so that a task waits and nothing blocks. See "Streams
//! and sockets" in `docs/spec/targets.md`.
//!
//! Each stream has a thread that reads when it is asked to, and one
//! that writes. Only the program's thread hands over what they got.

use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::sync::mpsc::{Sender, channel};
use std::thread;

use super::net::Reached;
use super::pending::Sent;
use super::sockets::Came;
use super::system::failure;
use super::waits::Chunk;
use super::{Failure, Kind};

/// The two directions of something a stream is made of, and what ends
/// both whichever thread is blocked in one.
pub(super) struct Halves {
    pub(super) read: Box<dyn Read + Send>,
    /// Nothing, for what is only read.
    pub(super) write: Option<Box<dyn Write + Send>>,
    pub(super) shut: Box<dyn Fn() + Send>,
}

/// What a stream's thread got for a ticket.
pub(super) enum Event {
    Read(Chunk),
    Wrote(Result<(), Failure>),
}

/// The most bytes one reading takes, whatever was asked for.
const PIECE: usize = 1 << 16;

struct Stream {
    asks: Sender<(u64, usize)>,
    writes: Option<Sender<(u64, Vec<u8>)>>,
    /// What was read for a wait its task left: the next to read gets it.
    unread: VecDeque<Chunk>,
    shut: Box<dyn Fn() + Send>,
}

impl Drop for Stream {
    /// Ends the stream, which is what lets its threads go.
    fn drop(&mut self) {
        (self.shut)();
    }
}

pub(super) struct Streams {
    sender: Sender<Sent>,
    held: HashMap<i64, Stream>,
    /// The stream each ticket waits on, and whether it waits to read.
    waiting: HashMap<u64, (i64, bool)>,
    numbered: i64,
}

fn closed() -> Failure {
    Failure {
        kind: Kind::Failed,
        message: "the stream is closed".to_owned(),
    }
}

/// Reads once, however often the system says to try again.
fn piece(read: &mut dyn Read, most: usize) -> Chunk {
    let mut piece = vec![0; most.clamp(1, PIECE)];
    loop {
        match read.read(&mut piece) {
            Ok(0) => return Ok(None),
            Ok(count) => {
                piece.truncate(count);
                return Ok(Some(piece));
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(failure(&error)),
        }
    }
}

/// At most `most` bytes of what was read, with the rest kept first
/// among what is unread.
fn cut(chunk: Chunk, most: usize, unread: &mut VecDeque<Chunk>) -> Chunk {
    match chunk {
        Ok(Some(mut bytes)) if bytes.len() > most.max(1) => {
            let rest = bytes.split_off(most.max(1));
            unread.push_front(Ok(Some(rest)));
            Ok(Some(bytes))
        }
        chunk => chunk,
    }
}

impl Streams {
    pub(super) fn new(sender: Sender<Sent>) -> Self {
        Self {
            sender,
            held: HashMap::new(),
            waiting: HashMap::new(),
            numbered: 0,
        }
    }

    /// Whether a thread may yet send something a task waits for.
    pub(super) fn waits(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Holds a stream, and gives the number it is known by.
    pub(super) fn add(&mut self, halves: Halves) -> i64 {
        self.numbered += 1;
        let id = self.numbered;
        let Halves {
            mut read,
            write,
            shut,
        } = halves;
        let (asks, asked) = channel::<(u64, usize)>();
        let sender = self.sender.clone();
        // Ends when the stream is let go, or the program is gone.
        thread::spawn(move || {
            for (ticket, most) in asked {
                let event = Event::Read(piece(&mut *read, most));
                if sender
                    .send(Sent::Net(Came::Stream(id, ticket, event)))
                    .is_err()
                {
                    return;
                }
            }
        });
        let writes = write.map(|mut write| {
            let (writes, written) = channel::<(u64, Vec<u8>)>();
            let sender = self.sender.clone();
            thread::spawn(move || {
                for (ticket, data) in written {
                    let wrote: io::Result<()> = write.write_all(&data).and_then(|()| write.flush());
                    let event = Event::Wrote(wrote.map_err(|error| failure(&error)));
                    if sender
                        .send(Sent::Net(Came::Stream(id, ticket, event)))
                        .is_err()
                    {
                        return;
                    }
                }
            });
            writes
        });
        let stream = Stream {
            asks,
            writes,
            unread: VecDeque::new(),
            shut,
        };
        self.held.insert(id, stream);
        id
    }

    /// What a stream has next, if it was read already or the stream
    /// is closed; otherwise the ticket waits for it.
    pub(super) fn read(&mut self, ticket: u64, id: i64, most: i64) -> Option<Reached> {
        let most = usize::try_from(most).unwrap_or(1);
        let Some(stream) = self.held.get_mut(&id) else {
            return Some(Reached::Read(Ok(None)));
        };
        if let Some(chunk) = stream.unread.pop_front() {
            return Some(Reached::Read(cut(chunk, most, &mut stream.unread)));
        }
        self.waiting.insert(ticket, (id, true));
        let _ = stream.asks.send((ticket, most));
        None
    }

    /// Why nothing can be written, if nothing can; otherwise the
    /// ticket waits until it is.
    pub(super) fn write(&mut self, ticket: u64, id: i64, data: Vec<u8>) -> Option<Reached> {
        let Some(stream) = self.held.get(&id) else {
            return Some(Reached::Wrote(Err(closed())));
        };
        let Some(writes) = &stream.writes else {
            return Some(Reached::Wrote(Err(Failure {
                kind: Kind::Invalid,
                message: "the stream is only read".to_owned(),
            })));
        };
        self.waiting.insert(ticket, (id, false));
        let _ = writes.send((ticket, data));
        None
    }

    /// Hands what a thread got to the ticket it was got for. What was
    /// read for a task that left is kept for whoever reads next.
    pub(super) fn took(&mut self, id: i64, ticket: u64, event: Event) -> Option<(u64, Reached)> {
        let waited = self.waiting.get(&ticket).is_some_and(|(on, _)| *on == id);
        if waited {
            self.waiting.remove(&ticket);
        }
        match event {
            Event::Read(chunk) if waited => Some((ticket, Reached::Read(chunk))),
            Event::Wrote(wrote) if waited => Some((ticket, Reached::Wrote(wrote))),
            Event::Read(chunk) => {
                let stream = self.held.get_mut(&id)?;
                stream.unread.push_back(chunk);
                None
            }
            Event::Wrote(_) => None,
        }
    }

    /// Closes a stream, and answers whatever waited on it.
    pub(super) fn close(&mut self, id: i64) -> Vec<(u64, Reached)> {
        self.held.remove(&id);
        let mut left = Vec::new();
        self.waiting.retain(|ticket, (on, reads)| {
            if *on != id {
                return true;
            }
            let reached = if *reads {
                Reached::Read(Ok(None))
            } else {
                Reached::Wrote(Err(closed()))
            };
            left.push((*ticket, reached));
            false
        });
        left.sort_by_key(|(ticket, _)| *ticket);
        left
    }

    /// Stops a ticket waiting. What is being read for it is read
    /// still, and kept.
    pub(super) fn forsake(&mut self, ticket: u64) {
        self.waiting.remove(&ticket);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use std::io::Cursor;
    use std::sync::mpsc::channel;

    use super::{Halves, Reached, Sent, Streams, cut};
    use crate::host::sockets::Came;

    #[test]
    fn what_is_read_for_a_task_that_left_is_given_to_the_next_to_read() {
        let (sender, sent) = channel();
        let mut streams = Streams::new(sender);
        let stream = streams.add(Halves {
            read: Box::new(Cursor::new(b"kept".to_vec())),
            write: None,
            shut: Box::new(|| {}),
        });

        assert_eq!(streams.read(1, stream, 8), None);
        streams.forsake(1);
        let Ok(Sent::Net(Came::Stream(of, ticket, event))) = sent.recv() else {
            panic!("the stream's thread sent nothing");
        };
        assert_eq!(streams.took(of, ticket, event), None);

        let kept = Reached::Read(Ok(Some(b"kept".to_vec())));
        assert_eq!(streams.read(2, stream, 8), Some(kept));
    }

    #[test]
    fn what_was_read_is_given_at_most_as_many_bytes_at_a_time_as_are_asked_for() {
        let mut unread = VecDeque::new();

        let first = cut(Ok(Some(b"hello".to_vec())), 2, &mut unread);

        assert_eq!(first, Ok(Some(b"he".to_vec())));
        assert_eq!(unread, [Ok(Some(b"llo".to_vec()))]);
    }
}
