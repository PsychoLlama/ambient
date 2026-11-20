//! How a real process reaches an address: what a host and a port
//! name, the connection made to one, and why there is none. See
//! "Streams and sockets" in `docs/spec/targets.md`.

use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};

use super::net::Address;
use super::streams::Halves;
use super::{Failure, Kind};

pub(super) fn failed(kind: Kind, message: &dyn ToString) -> Failure {
    Failure {
        kind,
        message: message.to_string(),
    }
}

/// Why nothing was connected to: whatever is not the program's doing
/// is that nothing answered.
pub(super) fn refused(error: &io::Error) -> Failure {
    let kind = match error.kind() {
        ErrorKind::NotFound => Kind::NotFound,
        ErrorKind::PermissionDenied => Kind::Denied,
        ErrorKind::InvalidInput => Kind::Invalid,
        _ => Kind::Unreachable,
    };
    failed(kind, error)
}

pub(super) fn port(port: i64) -> Result<u16, Failure> {
    u16::try_from(port).map_err(|_| failed(Kind::Invalid, &"no such port"))
}

/// The address a host and a port name, as the system finds it.
pub(super) fn address(host: &str, port_of: i64) -> Result<SocketAddr, Failure> {
    let found = (host, port(port_of)?).to_socket_addrs();
    let mut found = found.map_err(|error| failed(Kind::Invalid, &error))?;
    found
        .next()
        .ok_or_else(|| failed(Kind::Invalid, &"no such address"))
}

/// Where to send to reach what is bound at `bound` from this machine.
pub(super) fn reaching(mut bound: SocketAddr) -> SocketAddr {
    if bound.ip().is_unspecified() {
        bound.set_ip(match bound {
            SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    bound
}

pub(super) fn tcp(stream: TcpStream) -> io::Result<Halves> {
    // What is written is sent, not held for more to go with it.
    stream.set_nodelay(true)?;
    let (read, write) = (stream.try_clone()?, stream.try_clone()?);
    Ok(Halves {
        read: Box::new(read),
        write: Some(Box::new(write)),
        shut: Box::new(move || drop(stream.shutdown(Shutdown::Both))),
    })
}

#[cfg(unix)]
pub(super) mod unix {
    use std::io;
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::Halves;

    pub(in crate::host) fn halves(stream: UnixStream) -> io::Result<Halves> {
        let (read, write) = (stream.try_clone()?, stream.try_clone()?);
        Ok(Halves {
            read: Box::new(read),
            write: Some(Box::new(write)),
            shut: Box::new(move || drop(stream.shutdown(Shutdown::Both))),
        })
    }

    pub(in crate::host) fn connect(path: &str) -> io::Result<Halves> {
        halves(UnixStream::connect(path)?)
    }

    /// What accepts at a path, and what lets go of one blocked there
    /// and takes the socket away.
    pub(in crate::host) type Listening = (
        Box<dyn FnMut() -> io::Result<Halves> + Send>,
        Box<dyn Fn() + Send>,
    );

    pub(in crate::host) fn listen(path: &str) -> io::Result<Listening> {
        let listener = UnixListener::bind(path)?;
        let path = path.to_owned();
        let accept = move || halves(listener.accept()?.0);
        let wake = move || {
            drop(UnixStream::connect(&path));
            drop(std::fs::remove_file(&path));
        };
        Ok((Box::new(accept), Box::new(wake)))
    }
}

#[cfg(not(unix))]
pub(super) mod unix {
    use std::io;

    use super::Halves;

    pub(in crate::host) type Listening = (
        Box<dyn FnMut() -> io::Result<Halves> + Send>,
        Box<dyn Fn() + Send>,
    );

    fn none<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no Unix sockets here",
        ))
    }

    pub(in crate::host) fn connect(_path: &str) -> io::Result<Halves> {
        none()
    }

    pub(in crate::host) fn listen(_path: &str) -> io::Result<Listening> {
        none()
    }
}

pub(super) fn connect(address: &Address) -> Result<Halves, Failure> {
    let made = match address {
        Address::Tcp(host, port_of) => {
            let port = port(*port_of)?;
            TcpStream::connect((host.as_str(), port)).and_then(tcp)
        }
        Address::Unix(path) => unix::connect(path),
    };
    made.map_err(|error| refused(&error))
}
