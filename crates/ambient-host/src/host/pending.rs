//! What a real process is waiting on: timers, commands that are
//! running, and a line of input.
//!
//! Only this thread touches the program, so whatever blocks does so on
//! a thread of its own and sends what it got. A timer needs no thread:
//! waiting for the next outcome gives up when the soonest one is due.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::Write;
use std::io::{BufRead, Read};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::task::Poll;
use std::thread;
use std::time::{Duration, Instant};

use super::network::Network;
use super::signals::{Signal, Signals};
use super::sockets::{Came, Reaches};
use super::system::failure;
use super::waits::{Chunk, Done, Line, Work};
use super::watching::{Change, Watches};
use super::{Command, Failure, Held, Kind, Open, Opened, Ran, Response};
use crate::native::Posts;

/// A response whose body is still to arrive: its status, its headers,
/// and what the body is read from.
pub(super) type Opening = (i64, Vec<(String, String)>, Box<dyn Read + Send>);

/// What a thread sends back.
pub(super) enum Sent {
    /// A thread of a native bundle posted an answer.
    Posted,
    /// What a command wrote to its streams, once both were closed.
    Wrote(u64, Vec<u8>, Vec<u8>),
    /// A line of input, read for a ticket.
    Line(u64, Line),
    /// Bytes of input, read for a ticket.
    Chunk(u64, Chunk),
    /// What came of a request that was sent.
    Response(u64, Result<Response, Failure>),
    /// The head of a response, and what its body is read from.
    Opened(u64, Result<Opening, Failure>),
    /// A request a server was sent.
    Request(i64, tiny_http::Request),
    /// Something of a stream or a socket.
    Net(Came),
    /// The process was asked something from outside.
    Signal(Signal),
    /// A change a watcher saw, or word that it missed one.
    Changed(i64, Option<Change>),
}

pub(super) struct Pending {
    sender: Sender<Sent>,
    sent: Receiver<Sent>,
    /// Outcomes that need no more waiting.
    ready: VecDeque<(u64, Done)>,
    /// When each sleep is over, soonest first; the ticket breaks ties.
    timers: BTreeSet<(Instant, u64)>,
    /// The commands that are running.
    children: HashMap<u64, Child>,
    /// The tickets waiting for a line, and whom to ask for the next.
    /// A ticket is asked for with the most bytes it takes, or nothing
    /// if it takes a line.
    readers: Vec<u64>,
    lines: Option<Sender<(u64, Option<usize>)>>,
    /// Lines read for a wait its task left: the next to ask gets them.
    unread: VecDeque<Line>,
    /// Bytes read for a wait its task left, likewise.
    unread_bytes: VecDeque<Chunk>,
    pub(super) network: Network,
    /// Its streams and sockets.
    pub(super) reaches: Reaches,
    /// What waits to be told what the process is asked.
    signals: Signals,
    watches: Watches,
    /// What threads of native bundles post, which wake this.
    posts: Option<Arc<Posts>>,
}

impl Default for Pending {
    fn default() -> Self {
        let (sender, sent) = channel();
        Self {
            network: Network::new(sender.clone()),
            reaches: Reaches::new(sender.clone()),
            signals: Signals::default(),
            watches: Watches::default(),
            sender,
            sent,
            ready: VecDeque::new(),
            timers: BTreeSet::new(),
            children: HashMap::new(),
            readers: Vec::new(),
            lines: None,
            unread: VecDeque::new(),
            unread_bytes: VecDeque::new(),
            posts: None,
        }
    }
}

impl Pending {
    /// Has a post wake whoever waits here.
    pub(super) fn attend(&mut self, posts: Arc<Posts>) {
        let sender = self.sender.clone();
        posts.wake_with(Box::new(move || drop(sender.send(Sent::Posted))));
        self.posts = Some(posts);
    }

    pub(super) fn begin(&mut self, ticket: u64, work: Work) {
        match work {
            Work::Sleep(milliseconds) => {
                let length = Duration::from_millis(u64::try_from(milliseconds).unwrap_or(0));
                self.timers.insert((Instant::now() + length, ticket));
            }
            Work::Run(command) => self.run(ticket, command),
            Work::ReadLine => self.read_line(ticket),
            Work::Read(most) => self.read(ticket, usize::try_from(most).unwrap_or(0)),
            Work::Send(request) => self.network.send(ticket, request),
            Work::Open(request, trust) => {
                if let Err(failure) = self.network.open(ticket, request, trust) {
                    self.ready.push_back((ticket, Done::Opened(Err(failure))));
                }
            }
            Work::Accept(server) => {
                let done = self.network.accept(ticket, server);
                self.ready.extend(done.map(|done| (ticket, done)));
            }
            Work::Change(watcher) => {
                if let Poll::Ready(change) = self.watches.next(ticket, watcher) {
                    self.ready.push_back((ticket, Done::Changed(change)));
                }
            }
            Work::Signal => {
                let asked = self.signals.wait(ticket, &self.sender);
                self.ready
                    .extend(asked.map(|signal| (ticket, Done::Signalled(signal))));
            }
            Work::Reach(reach) => {
                let reached = self.reaches.begin(ticket, reach);
                self.ready
                    .extend(reached.map(|reached| (ticket, Done::Reached(reached))));
            }
        }
    }

    /// Starts a command. Threads read what it writes, so it is never
    /// held up by a full pipe; the command itself stays here, where it
    /// can be ended.
    fn run(&mut self, ticket: u64, command: Command) {
        let piped = || Stdio::piped();
        let mut process = std::process::Command::new(&command.program);
        process.args(&command.args).envs(command.variables);
        if let Some(directory) = &command.directory {
            process.current_dir(directory);
        }
        let spawned = process
            .stdin(piped())
            .stdout(piped())
            .stderr(piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let refused = Done::Ran(Err(failure(&error)));
                return self.ready.push_back((ticket, refused));
            }
        };
        let (out, err) = (child.stdout.take(), child.stderr.take());
        // Written on a thread of its own, and then closed: a command
        // that reads none of it must not hold up what it writes.
        if let Some(mut input) = child.stdin.take() {
            thread::spawn(move || input.write_all(&command.input));
        }
        let sender = self.sender.clone();
        thread::spawn(move || {
            let errors = thread::spawn(move || written(err));
            let out = written(out);
            let err = errors.join().unwrap_or_default();
            let _ = sender.send(Sent::Wrote(ticket, out, err));
        });
        self.children.insert(ticket, child);
    }

    /// Asks the thread that reads input for one more line.
    fn read_line(&mut self, ticket: u64) {
        if let Some(line) = self.unread.pop_front() {
            return self.ready.push_back((ticket, Done::Line(line)));
        }
        self.ask(ticket, None);
    }

    /// Asks the thread that reads input for what is next, as bytes.
    /// Bytes read for a wait its task left are given whole, however
    /// many were asked for: they were read once and are not read again.
    fn read(&mut self, ticket: u64, most: usize) {
        if let Some(chunk) = self.unread_bytes.pop_front() {
            return self.ready.push_back((ticket, Done::Read(chunk)));
        }
        self.ask(ticket, Some(most));
    }

    /// Asks the thread that reads input to read for a ticket: a line,
    /// or at most so many bytes. There is one such thread, so input is
    /// never read out of order.
    fn ask(&mut self, ticket: u64, most: Option<usize>) {
        self.readers.push(ticket);
        let sender = self.sender.clone();
        let lines = self.lines.get_or_insert_with(|| {
            let (asks, asked) = channel::<(u64, Option<usize>)>();
            thread::spawn(move || {
                for (ticket, most) in asked {
                    let input = &mut std::io::stdin().lock();
                    let _ = sender.send(match most {
                        None => Sent::Line(ticket, line(input)),
                        Some(most) => Sent::Chunk(ticket, chunk(input, most)),
                    });
                }
            });
            asks
        });
        let _ = lines.send((ticket, most));
    }

    /// Whether a ticket still waits for what was read for it, which it
    /// then does not.
    fn reads(&mut self, ticket: u64) -> bool {
        let at = self.readers.iter().position(|reader| *reader == ticket);
        at.map(|at| self.readers.remove(at)).is_some()
    }

    pub(super) fn next(&mut self) -> Option<(u64, Done)> {
        loop {
            if let Some(ready) = self.ready.pop_front() {
                return Some(ready);
            }
            let due = self.timers.first().copied();
            if let Some((when, ticket)) = due
                && when <= Instant::now()
            {
                self.timers.remove(&(when, ticket));
                return Some((ticket, Done::Slept));
            }
            let posts = self.posts.as_ref();
            // Nothing is given once a post is in: the program takes
            // it, and asks again.
            if posts.is_some_and(|posts| posts.ready()) {
                return None;
            }
            let threads = !self.children.is_empty()
                || !self.readers.is_empty()
                || self.network.waits()
                || self.reaches.waits()
                || self.signals.waits()
                || self.watches.waits()
                || posts.is_some_and(|posts| posts.coming());
            let sent = match due {
                Some((when, _)) if threads => match self.sent.recv_timeout(when - Instant::now()) {
                    Ok(sent) => sent,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return None,
                },
                Some((when, _)) => {
                    thread::sleep(when.saturating_duration_since(Instant::now()));
                    continue;
                }
                None if threads => self.sent.recv().ok()?,
                None => return None,
            };
            self.took(sent);
        }
    }

    /// Makes an outcome of what a thread sent, unless the task that
    /// waited for it has left.
    fn took(&mut self, sent: Sent) {
        match sent {
            // Woken: `next` looks at the posts itself.
            Sent::Posted => {}
            Sent::Wrote(ticket, out, err) => {
                let Some(mut child) = self.children.remove(&ticket) else {
                    return;
                };
                let code = child.wait().ok().and_then(|status| status.code());
                let code = code.map_or(-1, i64::from);
                let ran = Ran { code, out, err };
                self.ready.push_back((ticket, Done::Ran(Ok(ran))));
            }
            Sent::Line(ticket, line) => {
                if self.reads(ticket) {
                    self.ready.push_back((ticket, Done::Line(line)));
                } else {
                    self.unread.push_back(line);
                }
            }
            Sent::Chunk(ticket, chunk) => {
                if self.reads(ticket) {
                    self.ready.push_back((ticket, Done::Read(chunk)));
                } else {
                    self.unread_bytes.push_back(chunk);
                }
            }
            Sent::Response(ticket, response) => {
                if self.network.sent(ticket) {
                    self.ready.push_back((ticket, Done::Sent(response)));
                }
            }
            Sent::Opened(ticket, opened) => {
                if self.network.sent(ticket) {
                    let opened = opened.map(|opening| self.reaches.streamed(opening));
                    self.ready.push_back((ticket, Done::Opened(opened)));
                }
            }
            Sent::Request(server, request) => {
                self.ready.extend(self.network.arrived(server, request));
            }
            Sent::Changed(watcher, change) => {
                let came = self.watches.came(watcher, change);
                self.ready
                    .extend(came.map(|(ticket, change)| (ticket, Done::Changed(Some(change)))));
            }
            Sent::Signal(signal) => {
                let told = self.signals.came(signal);
                self.ready
                    .extend(told.map(|(ticket, signal)| (ticket, Done::Signalled(signal))));
            }
            Sent::Net(came) => {
                let reached = self.reaches.took(came);
                self.ready
                    .extend(reached.map(|(ticket, reached)| (ticket, Done::Reached(reached))));
            }
        }
    }

    /// Stops a server, and answers whatever waited to accept from it.
    pub(super) fn close(&mut self, server: i64) {
        let waiting = self.network.close(server);
        let closed = |ticket| (ticket, Done::Accepted(None));
        self.ready.extend(waiting.into_iter().map(closed));
    }

    /// Opens a listener, a socket, or a watcher.
    pub(super) fn open(&mut self, what: Open) -> Result<Opened, Failure> {
        match what {
            Open::Watch(path) => {
                let id = self.watches.watch(&path, self.sender.clone())?;
                Ok(Opened { id, port: 0 })
            }
            what => self.reaches.open(what),
        }
    }

    /// Lets go of a stream, a listener, or a socket, and answers
    /// whatever waited on it.
    pub(super) fn shut(&mut self, held: Held) {
        if let Held::Watcher(watcher) = held {
            let closed = |ticket| (ticket, Done::Changed(None));
            return self
                .ready
                .extend(self.watches.close(watcher).into_iter().map(closed));
        }
        let reached = self.reaches.shut(held).into_iter();
        self.ready
            .extend(reached.map(|(ticket, reached)| (ticket, Done::Reached(reached))));
    }

    /// Stops what a ticket was waiting on, where it can be stopped: a
    /// command is ended, a timer dropped. A line being read is kept
    /// for whoever asks next.
    pub(super) fn forsake(&mut self, ticket: u64) {
        self.ready.retain(|(ready, _)| *ready != ticket);
        self.timers.retain(|(_, timer)| *timer != ticket);
        self.readers.retain(|reader| *reader != ticket);
        self.network.forsake(ticket);
        self.reaches.forsake(ticket);
        self.signals.forsake(ticket);
        self.watches.forsake(ticket);
        if let Some(mut child) = self.children.remove(&ticket) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Everything a stream holds.
fn written(stream: Option<impl Read>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        let _ = stream.read_to_end(&mut bytes);
    }
    bytes
}

/// The next line of input, without its ending, or nothing at its end.
/// One that is not text is not made text: it is a failure.
fn line(input: &mut impl BufRead) -> Line {
    let mut line = Vec::new();
    let read = input.read_until(b'\n', &mut line);
    let failed = |message: String| Failure {
        kind: Kind::Invalid,
        message,
    };
    if read.map_err(|error| failed(error.to_string()))? == 0 {
        return Ok(None);
    }
    let mut line = String::from_utf8(line)
        .map_err(|_| failed("a line of standard input is not text".to_owned()))?;
    let ending = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(ending);
    Ok(Some(line))
}

/// What input has next: at least one byte and at most `most`, or
/// nothing at its end. It takes what a line read before it left in the
/// buffer, and does not wait for more than is there.
fn chunk(input: &mut impl BufRead, most: usize) -> Chunk {
    let failed = |error: std::io::Error| Failure {
        kind: Kind::Failed,
        message: error.to_string(),
    };
    let held = input.fill_buf().map_err(failed)?;
    let taken = held.len().min(most.max(1));
    let bytes = held[..taken].to_vec();
    input.consume(taken);
    Ok((!bytes.is_empty()).then_some(bytes))
}

#[cfg(test)]
mod tests {
    use super::{Kind, chunk, line};

    #[test]
    fn bytes_are_read_after_a_line_and_up_to_the_end() {
        let mut input = &b"one\n\xff\xfe\x00"[..];

        assert_eq!(line(&mut input), Ok(Some("one".to_owned())));
        assert_eq!(chunk(&mut input, 2), Ok(Some(vec![0xff, 0xfe])));
        assert_eq!(chunk(&mut input, 9), Ok(Some(vec![0])));
        assert_eq!(chunk(&mut input, 9), Ok(None));
    }

    #[test]
    fn a_line_that_is_not_text_is_neither_text_nor_the_end() {
        let mut input = &b"one\r\n\xff\xfe\nlast"[..];
        let mut lines = std::iter::from_fn(|| Some(line(&mut input)));

        assert_eq!(lines.next(), Some(Ok(Some("one".to_owned()))));
        let garbled = lines.next().and_then(Result::err);
        assert_eq!(garbled.map(|failure| failure.kind), Some(Kind::Invalid));
        // What follows is still there to read.
        assert_eq!(lines.next(), Some(Ok(Some("last".to_owned()))));
        assert_eq!(lines.next(), Some(Ok(None)));
    }
}
