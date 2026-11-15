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
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use super::network::Network;
use super::system::failure;
use super::waits::{Done, Work};
use super::{Command, Failure, Ran, Response};

/// What a thread sends back.
pub(super) enum Sent {
    /// What a command wrote to its streams, once both were closed.
    Wrote(u64, Vec<u8>, Vec<u8>),
    /// A line of input, read for a ticket.
    Line(u64, Option<String>),
    /// What came of a request that was sent.
    Response(u64, Result<Response, Failure>),
    /// A request a server was sent.
    Request(i64, tiny_http::Request),
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
    readers: Vec<u64>,
    lines: Option<Sender<u64>>,
    /// Lines read for a wait its task left: the next to ask gets them.
    unread: VecDeque<Option<String>>,
    pub(super) network: Network,
}

impl Default for Pending {
    fn default() -> Self {
        let (sender, sent) = channel();
        Self {
            network: Network::new(sender.clone()),
            sender,
            sent,
            ready: VecDeque::new(),
            timers: BTreeSet::new(),
            children: HashMap::new(),
            readers: Vec::new(),
            lines: None,
            unread: VecDeque::new(),
        }
    }
}

impl Pending {
    pub(super) fn begin(&mut self, ticket: u64, work: Work) {
        match work {
            Work::Sleep(milliseconds) => {
                let length = Duration::from_millis(u64::try_from(milliseconds).unwrap_or(0));
                self.timers.insert((Instant::now() + length, ticket));
            }
            Work::Run(command) => self.run(ticket, command),
            Work::ReadLine => self.read_line(ticket),
            Work::Send(request) => self.network.send(ticket, request),
            Work::Accept(server) => {
                let done = self.network.accept(ticket, server);
                self.ready.extend(done.map(|done| (ticket, done)));
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

    /// Asks the thread that reads input for one more line. There is one
    /// such thread, so lines are never read out of order.
    fn read_line(&mut self, ticket: u64) {
        if let Some(line) = self.unread.pop_front() {
            return self.ready.push_back((ticket, Done::Line(line)));
        }
        self.readers.push(ticket);
        let sender = self.sender.clone();
        let lines = self.lines.get_or_insert_with(|| {
            let (asks, asked) = channel::<u64>();
            thread::spawn(move || {
                for ticket in asked {
                    let _ = sender.send(Sent::Line(ticket, line()));
                }
            });
            asks
        });
        let _ = lines.send(ticket);
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
            let threads =
                !self.children.is_empty() || !self.readers.is_empty() || self.network.waits();
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
                if let Some(at) = self.readers.iter().position(|reader| *reader == ticket) {
                    self.readers.remove(at);
                    self.ready.push_back((ticket, Done::Line(line)));
                } else {
                    self.unread.push_back(line);
                }
            }
            Sent::Response(ticket, response) => {
                if self.network.sent(ticket) {
                    self.ready.push_back((ticket, Done::Sent(response)));
                }
            }
            Sent::Request(server, request) => {
                self.ready.extend(self.network.arrived(server, request));
            }
        }
    }

    /// Stops a server, and answers whatever waited to accept from it.
    pub(super) fn close(&mut self, server: i64) {
        let waiting = self.network.close(server);
        let closed = |ticket| (ticket, Done::Accepted(None));
        self.ready.extend(waiting.into_iter().map(closed));
    }

    /// Stops what a ticket was waiting on, where it can be stopped: a
    /// command is ended, a timer dropped. A line being read is kept
    /// for whoever asks next.
    pub(super) fn forsake(&mut self, ticket: u64) {
        self.ready.retain(|(ready, _)| *ready != ticket);
        self.timers.retain(|(_, timer)| *timer != ticket);
        self.readers.retain(|reader| *reader != ticket);
        self.network.forsake(ticket);
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

/// The next line of standard input, without its ending.
fn line() -> Option<String> {
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line).ok()?;
    if read == 0 {
        return None;
    }
    let ending = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(ending);
    Some(line)
}
