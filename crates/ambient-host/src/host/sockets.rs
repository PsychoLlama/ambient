//! Sockets for a real process: connections made and accepted on
//! threads, and datagrams a thread each brings to the program. See
//! "Streams and sockets" in `docs/spec/targets.md`.
//!
//! The same kind of waiting as [`super::network`]: a thread blocks,
//! and sends what it got. A thread blocked in a listener or a socket
//! is let go by being sent something once it is marked closed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use super::net::{Address, Datagram, Held, Open, Opened, Reach, Reached};
use super::pending::Sent;
use super::reaching::{address, connect, failed, port, reaching, refused, tcp, unix};
use super::streams::{Event, Halves, Streams};
use super::system::failure;
use super::{Failure, Kind};

/// What a thread of a socket or a stream sends back.
pub(super) enum Came {
    /// What a stream's thread got for a ticket.
    Stream(i64, u64, Event),
    /// What came of connecting, for a ticket.
    Connected(u64, Result<Halves, Failure>),
    /// A connection a listener was sent.
    Arrived(i64, Halves),
    /// A datagram a socket was sent.
    Datagram(i64, Datagram),
}

/// The most connections a listener holds that nothing has accepted,
/// and the most datagrams a socket holds that nothing has received.
const HELD: usize = 256;

/// What waits on something a thread brings, and what it brought that
/// nothing has taken.
struct Queue<T> {
    untaken: VecDeque<T>,
    waiting: VecDeque<u64>,
    /// How many its thread has brought that nothing has taken: the
    /// thread drops one more than [`HELD`].
    held: Arc<AtomicUsize>,
    /// Marks it closed and lets its thread go.
    stop: Box<dyn Fn() + Send>,
}

impl<T> Queue<T> {
    fn new(held: Arc<AtomicUsize>, stop: Box<dyn Fn() + Send>) -> Self {
        Self {
            untaken: VecDeque::new(),
            waiting: VecDeque::new(),
            held,
            stop,
        }
    }

    /// The next that was brought, or the ticket waits for one.
    fn take(&mut self, ticket: u64) -> Option<T> {
        let next = self.untaken.pop_front();
        if next.is_some() {
            self.held.fetch_sub(1, Ordering::Relaxed);
        } else {
            self.waiting.push_back(ticket);
        }
        next
    }

    /// Hands what was brought to a ticket that waits, or keeps it.
    fn bring(&mut self, brought: T) -> Option<(u64, T)> {
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
fn room(held: &AtomicUsize) -> bool {
    let has = held.load(Ordering::Relaxed) < HELD;
    if has {
        held.fetch_add(1, Ordering::Relaxed);
    }
    has
}

pub(super) struct Reaches {
    sender: Sender<Sent>,
    pub(super) streams: Streams,
    /// The tickets of the connections that are not yet made.
    connecting: HashSet<u64>,
    listeners: HashMap<i64, Queue<Halves>>,
    sockets: HashMap<i64, (UdpSocket, Queue<Datagram>)>,
    numbered: i64,
}

impl Reaches {
    pub(super) fn new(sender: Sender<Sent>) -> Self {
        Self {
            streams: Streams::new(sender.clone()),
            sender,
            connecting: HashSet::new(),
            listeners: HashMap::new(),
            sockets: HashMap::new(),
            numbered: 0,
        }
    }

    fn number(&mut self) -> i64 {
        self.numbered += 1;
        self.numbered
    }

    /// Whether a thread may yet send something a task waits for.
    pub(super) fn waits(&self) -> bool {
        !self.connecting.is_empty()
            || self.streams.waits()
            || self
                .listeners
                .values()
                .any(|queue| !queue.waiting.is_empty())
            || self
                .sockets
                .values()
                .any(|(_, queue)| !queue.waiting.is_empty())
    }

    /// Begins what a ticket asked for, and gives what came of it if
    /// that took no waiting.
    pub(super) fn begin(&mut self, ticket: u64, reach: Reach) -> Option<Reached> {
        match reach {
            Reach::Connect(address) => {
                self.connecting.insert(ticket);
                let sender = self.sender.clone();
                thread::spawn(move || {
                    let came = Came::Connected(ticket, connect(&address));
                    let _ = sender.send(Sent::Net(came));
                });
                None
            }
            Reach::Take(listener) => {
                let Some(queue) = self.listeners.get_mut(&listener) else {
                    return Some(Reached::Took(None));
                };
                let halves = queue.take(ticket)?;
                Some(Reached::Took(Some(self.streams.add(halves))))
            }
            Reach::Read(stream, most) => self.streams.read(ticket, stream, most),
            Reach::Write(stream, data) => self.streams.write(ticket, stream, data),
            Reach::SendTo {
                socket,
                data,
                host,
                port,
            } => Some(Reached::Did(self.send_to(socket, &data, &host, port))),
            Reach::Join(socket, group) => Some(Reached::Did(self.join(socket, &group))),
            Reach::Receive(socket) => {
                let Some((_, queue)) = self.sockets.get_mut(&socket) else {
                    return Some(Reached::Received(None));
                };
                queue
                    .take(ticket)
                    .map(|datagram| Reached::Received(Some(datagram)))
            }
        }
    }

    /// Makes an outcome of what a thread sent, unless the task that
    /// waited for it has left.
    pub(super) fn took(&mut self, came: Came) -> Option<(u64, Reached)> {
        match came {
            Came::Stream(stream, ticket, event) => self.streams.took(stream, ticket, event),
            Came::Connected(ticket, made) => {
                if !self.connecting.remove(&ticket) {
                    // Nothing waits for it: it is ended.
                    if let Ok(halves) = made {
                        (halves.shut)();
                    }
                    return None;
                }
                let made = made.map(|halves| self.streams.add(halves));
                Some((ticket, Reached::Connected(made)))
            }
            Came::Arrived(listener, halves) => {
                let Some(queue) = self.listeners.get_mut(&listener) else {
                    (halves.shut)();
                    return None;
                };
                let (ticket, halves) = queue.bring(halves)?;
                Some((ticket, Reached::Took(Some(self.streams.add(halves)))))
            }
            Came::Datagram(socket, datagram) => {
                let (_, queue) = self.sockets.get_mut(&socket)?;
                let (ticket, datagram) = queue.bring(datagram)?;
                Some((ticket, Reached::Received(Some(datagram))))
            }
        }
    }

    pub(super) fn open(&mut self, what: Open) -> Result<Opened, Failure> {
        match what {
            Open::Listen(Address::Tcp(host, port_of)) => self.listen(&host, port_of),
            Open::Listen(Address::Unix(path)) => {
                let (accept, wake) = unix::listen(&path).map_err(|error| failure(&error))?;
                Ok(self.accepting(accept, wake, 0))
            }
            Open::Bind { host, port, shared } => self.bind(&host, port, shared),
        }
    }

    fn listen(&mut self, host: &str, port_of: i64) -> Result<Opened, Failure> {
        let listener = TcpListener::bind((host, port(port_of)?));
        let listener = listener.map_err(|error| failure(&error))?;
        let bound = listener.local_addr().map_err(|error| failure(&error))?;
        let accept = move || tcp(listener.accept()?.0);
        let wake = move || drop(TcpStream::connect(reaching(bound)));
        Ok(self.accepting(Box::new(accept), Box::new(wake), bound.port()))
    }

    /// Holds a listener whose thread brings what `accept` gives, until
    /// it is stopped: marked closed, and let go by `wake`.
    fn accepting(
        &mut self,
        mut accept: Box<dyn FnMut() -> io::Result<Halves> + Send>,
        wake: Box<dyn Fn() + Send>,
        port: u16,
    ) -> Opened {
        let id = self.number();
        let (held, closed) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (count, over, sender) = (Arc::clone(&held), Arc::clone(&closed), self.sender.clone());
        thread::spawn(move || {
            loop {
                let accepted = accept();
                if over.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(halves) = accepted else {
                    // Nothing to accept with just now: not a reason to spin.
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                if !room(&count) {
                    (halves.shut)();
                    continue;
                }
                if sender.send(Sent::Net(Came::Arrived(id, halves))).is_err() {
                    return;
                }
            }
        });
        let stop = move || {
            closed.store(true, Ordering::Relaxed);
            wake();
        };
        self.listeners.insert(id, Queue::new(held, Box::new(stop)));
        Opened {
            id,
            port: i64::from(port),
        }
    }

    fn bind(&mut self, host: &str, port_of: i64, shared: bool) -> Result<Opened, Failure> {
        use socket2::{Domain, Protocol, Socket, Type};
        let at = address(host, port_of)?;
        let bound = (|| {
            let socket = Socket::new(Domain::for_address(at), Type::DGRAM, Some(Protocol::UDP))?;
            if shared {
                socket.set_reuse_address(true)?;
                #[cfg(unix)]
                socket.set_reuse_port(true)?;
            }
            socket.bind(&at.into())?;
            let socket = UdpSocket::from(socket);
            Ok((
                socket.try_clone()?,
                socket.try_clone()?,
                socket.local_addr()?,
                socket,
            ))
        })();
        let (receiving, waking, at, socket) = bound.map_err(|error: io::Error| failure(&error))?;
        let id = self.number();
        let (held, closed) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (count, over, sender) = (Arc::clone(&held), Arc::clone(&closed), self.sender.clone());
        thread::spawn(move || {
            let mut data = vec![0; 1 << 16];
            loop {
                let received = receiving.recv_from(&mut data);
                if over.load(Ordering::Relaxed) {
                    return;
                }
                let Ok((length, from)) = received else {
                    continue;
                };
                if !room(&count) {
                    continue;
                }
                let datagram = Datagram {
                    data: data[..length].to_vec(),
                    host: from.ip().to_string(),
                    port: i64::from(from.port()),
                };
                if sender
                    .send(Sent::Net(Came::Datagram(id, datagram)))
                    .is_err()
                {
                    return;
                }
            }
        });
        let stop = move || {
            closed.store(true, Ordering::Relaxed);
            drop(waking.send_to(&[], reaching(at)));
        };
        self.sockets
            .insert(id, (socket, Queue::new(held, Box::new(stop))));
        Ok(Opened {
            id,
            port: i64::from(at.port()),
        })
    }

    fn socket(&self, socket: i64) -> Result<&UdpSocket, Failure> {
        let held = self.sockets.get(&socket);
        held.map(|(socket, _)| socket)
            .ok_or_else(|| failed(Kind::Failed, &"the socket is closed"))
    }

    fn send_to(&self, socket: i64, data: &[u8], host: &str, port: i64) -> Result<(), Failure> {
        let to = address(host, port)?;
        let sent = self.socket(socket)?.send_to(data, to);
        sent.map(|_| ()).map_err(|error| refused(&error))
    }

    fn join(&self, socket: i64, group: &str) -> Result<(), Failure> {
        let socket = self.socket(socket)?;
        let group: IpAddr = group
            .parse()
            .map_err(|_| failed(Kind::Invalid, &format!("`{group}` is no group")))?;
        let joined = match group {
            IpAddr::V4(group) => socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(group) => socket.join_multicast_v6(&group, 0),
        };
        joined.map_err(|error| failed(Kind::Invalid, &error))
    }

    /// Lets go of what the program held, and answers whatever waited
    /// on it.
    pub(super) fn shut(&mut self, held: Held) -> Vec<(u64, Reached)> {
        let none = |waiting: VecDeque<u64>, reached: fn() -> Reached| {
            waiting
                .into_iter()
                .map(|ticket| (ticket, reached()))
                .collect()
        };
        match held {
            Held::Stream(stream) => self.streams.close(stream),
            Held::Listener(listener) => self
                .listeners
                .remove(&listener)
                .map_or_else(Vec::new, |mut queue| {
                    none(std::mem::take(&mut queue.waiting), || Reached::Took(None))
                }),
            Held::Socket(socket) => {
                self.sockets
                    .remove(&socket)
                    .map_or_else(Vec::new, |(_, mut queue)| {
                        none(std::mem::take(&mut queue.waiting), || {
                            Reached::Received(None)
                        })
                    })
            }
        }
    }

    /// Stops a ticket waiting. A connection being made is ended when
    /// it is made.
    pub(super) fn forsake(&mut self, ticket: u64) {
        self.connecting.remove(&ticket);
        self.streams.forsake(ticket);
        let left = |waiting: &mut VecDeque<u64>| waiting.retain(|waiting| *waiting != ticket);
        self.listeners
            .values_mut()
            .for_each(|queue| left(&mut queue.waiting));
        self.sockets
            .values_mut()
            .for_each(|(_, queue)| left(&mut queue.waiting));
    }
}
