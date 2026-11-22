//! Streams and sockets as a world carries them, and as `system.stream`
//! and `system.net` describe them: what is asked for and what comes of
//! it, as values a program holds.

use std::collections::BTreeMap;

use super::http::{Handler, Starter, handle};
use super::waits::Chunk;
use super::{Failure, PACKAGE, Work, bytes, error, int, text};
use crate::value::Value;

/// Where a connection is made to, or listened for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    /// A host and a port, by TCP.
    Tcp(String, i64),
    /// A Unix socket, by its path.
    Unix(String),
}

/// Something a world is asked to open, which takes no time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// Listen for connections.
    Listen(Address),
    /// Bind a socket for datagrams, to a port other programs may bind
    /// too if it is `shared`.
    Bind {
        host: String,
        port: i64,
        shared: bool,
    },
    /// Watch a file, or a directory and all it holds.
    Watch(String),
}

/// What a world opened: the number it is known by, and its port, or
/// zero for what has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened {
    pub id: i64,
    pub port: i64,
}

/// Something a program holds of the world, by the number the world
/// gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Stream(i64),
    Listener(i64),
    Socket(i64),
    Watcher(i64),
}

/// What a socket received, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub data: Vec<u8>,
    pub host: String,
    pub port: i64,
}

/// Something a world is asked to begin on a stream or a socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    Connect(Address),
    /// Take the next connection a listener was sent.
    Take(i64),
    /// Read what a stream has next: at most this many bytes.
    Read(i64, i64),
    Write(i64, Vec<u8>),
    SendTo {
        socket: i64,
        data: Vec<u8>,
        host: String,
        port: i64,
    },
    /// Have a socket receive what is sent to a group.
    Join(i64, String),
    /// Take the next datagram a socket was sent.
    Receive(i64),
}

/// What came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reached {
    /// The stream, or why there is none.
    Connected(Result<i64, Failure>),
    /// The connection, a stream, or nothing once the listener is closed.
    Took(Option<i64>),
    /// The bytes, or nothing at the end of the stream.
    Read(Chunk),
    Wrote(Result<(), Failure>),
    /// What came of sending a datagram, or of joining a group.
    Did(Result<(), Failure>),
    /// The datagram, or nothing once the socket is closed.
    Received(Option<Datagram>),
}

/// What `system.stream` and `system.net` call the brands of what a
/// program holds.
pub(super) const STREAM: &str = "stream.Stream";
pub(super) const LISTENER: &str = "net.Listener";
pub(super) const SOCKET: &str = "net.Socket";

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

impl Reached {
    /// What the handler that began it gives its program.
    pub(super) fn value(self) -> Value {
        let done = |done: Result<(), Failure>, module| {
            done.map_or_else(|failure| error(module, &failure), |()| Value::Null)
        };
        match self {
            Self::Connected(made) => {
                made.map_or_else(|failure| error("net", &failure), |id| handle(STREAM, id))
            }
            Self::Took(stream) => stream.map_or(Value::Null, |id| handle(STREAM, id)),
            Self::Read(Ok(chunk)) => chunk.map_or(Value::Null, Value::Bytes),
            Self::Read(Err(failure)) => error("stream", &failure),
            Self::Wrote(wrote) => done(wrote, "stream"),
            Self::Did(did) => done(did, "net"),
            Self::Received(datagram) => datagram.map_or(Value::Null, |datagram| {
                record([
                    ("data", Value::Bytes(datagram.data)),
                    ("host", Value::String(datagram.host)),
                    ("port", Value::Int(datagram.port)),
                ])
            }),
        }
    }
}

/// What a world numbered that an argument is the handle for, under one
/// of `brands`, and which.
fn held<'a>(args: &[Value], position: usize, brands: &[&'a str]) -> Result<(&'a str, i64), String> {
    let Some(Value::Handle {
        package,
        brand,
        number,
    }) = args.get(position)
    else {
        return Err(format!("argument {position} is not a handle"));
    };
    let brand = brands
        .iter()
        .find(|known| package == PACKAGE && *known == brand);
    brand
        .map(|brand| (*brand, number.cast_signed()))
        .ok_or_else(|| {
            format!(
                "argument {position} is not a handle of `{}`",
                brands.join("` or `")
            )
        })
}

fn one(args: &[Value], position: usize, brand: &str) -> Result<i64, String> {
    Ok(held(args, position, &[brand])?.1)
}

fn flag(args: &[Value], position: usize) -> Result<bool, String> {
    match args.get(position) {
        Some(Value::Bool(flag)) => Ok(*flag),
        _ => Err(format!("argument {position} is not a boolean")),
    }
}

/// What is opened, as `system.net` describes it: its port beside the
/// handle for it, under `name`.
fn opened(opened: Result<Opened, Failure>, name: &str, brand: &str) -> Value {
    opened.map_or_else(
        |failure| error("net", &failure),
        |opened| {
            record([
                ("port", Value::Int(opened.port)),
                (name, handle(brand, opened.id)),
            ])
        },
    )
}

pub(super) const HANDLERS: [(&str, Handler); 5] = [
    ("stream.close", |world, args| {
        world.shut(Held::Stream(one(args, 0, STREAM)?));
        Ok(Value::Null)
    }),
    ("net.listen", |world, args| {
        let address = Address::Tcp(text(args, 0)?.to_owned(), int(args, 1)?);
        Ok(opened(
            world.open(Open::Listen(address)),
            "listener",
            LISTENER,
        ))
    }),
    ("net.listen_unix", |world, args| {
        let address = Address::Unix(text(args, 0)?.to_owned());
        let listening = world.open(Open::Listen(address));
        Ok(listening.map_or_else(
            |failure| error("net", &failure),
            |opened| handle(LISTENER, opened.id),
        ))
    }),
    ("net.bind", |world, args| {
        let (host, port, shared) = (text(args, 0)?.to_owned(), int(args, 1)?, flag(args, 2)?);
        Ok(opened(
            world.open(Open::Bind { host, port, shared }),
            "socket",
            SOCKET,
        ))
    }),
    ("net.close", |world, args| {
        let (brand, id) = held(args, 0, &[LISTENER, SOCKET])?;
        world.shut(if brand == LISTENER {
            Held::Listener(id)
        } else {
            Held::Socket(id)
        });
        Ok(Value::Null)
    }),
];

pub(super) const STARTERS: [(&str, Starter); 8] = [
    ("stream.read", |args| {
        Ok(Work::Reach(Reach::Read(
            one(args, 0, STREAM)?,
            int(args, 1)?,
        )))
    }),
    ("stream.write", |args| {
        let data = bytes(args, 1)?.to_vec();
        Ok(Work::Reach(Reach::Write(one(args, 0, STREAM)?, data)))
    }),
    ("net.connect", |args| {
        let address = Address::Tcp(text(args, 0)?.to_owned(), int(args, 1)?);
        Ok(Work::Reach(Reach::Connect(address)))
    }),
    ("net.connect_unix", |args| {
        let address = Address::Unix(text(args, 0)?.to_owned());
        Ok(Work::Reach(Reach::Connect(address)))
    }),
    ("net.accept", |args| {
        Ok(Work::Reach(Reach::Take(one(args, 0, LISTENER)?)))
    }),
    ("net.join", |args| {
        let group = text(args, 1)?.to_owned();
        Ok(Work::Reach(Reach::Join(one(args, 0, SOCKET)?, group)))
    }),
    ("net.send_to", |args| {
        let socket = one(args, 0, SOCKET)?;
        let (data, host, port) = (
            bytes(args, 1)?.to_vec(),
            text(args, 2)?.to_owned(),
            int(args, 3)?,
        );
        Ok(Work::Reach(Reach::SendTo {
            socket,
            data,
            host,
            port,
        }))
    }),
    ("net.receive", |args| {
        Ok(Work::Reach(Reach::Receive(one(args, 0, SOCKET)?)))
    }),
];

/// What the world is told when a program holds one of these no longer.
pub(super) fn dropped(brand: &str, number: i64) -> Option<Held> {
    match brand {
        STREAM => Some(Held::Stream(number)),
        LISTENER => Some(Held::Listener(number)),
        SOCKET => Some(Held::Socket(number)),
        _ => None,
    }
}
