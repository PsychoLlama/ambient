//! HTTP for a real process: requests sent on threads of their own, and
//! servers whose requests a thread each brings to the program.
//!
//! Only the program's thread answers a request or hands one over, so
//! what is here is the same kind of waiting as the rest of
//! [`super::pending`]: a thread blocks, and sends what it got.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;

use ureq::http;

use super::pending::Sent;
use super::waits::Done;
use super::{Failure, Incoming, Kind, Listening, Request, Response};

/// A server, the requests it was sent that nothing has accepted, and
/// the tickets waiting to accept one, each in the order it came.
struct Listener {
    server: Arc<tiny_http::Server>,
    unaccepted: VecDeque<Incoming>,
    waiting: VecDeque<u64>,
}

pub(super) struct Network {
    sender: Sender<Sent>,
    /// What sends requests, and keeps connections to send more on.
    agent: ureq::Agent,
    /// The tickets of the requests that have no response yet.
    sending: HashSet<u64>,
    listeners: HashMap<i64, Listener>,
    /// The requests servers accepted that have not been answered.
    unanswered: HashMap<i64, tiny_http::Request>,
    /// How many servers and requests have been numbered.
    numbered: i64,
}

fn failed(kind: Kind, message: &dyn ToString) -> Failure {
    Failure {
        kind,
        message: message.to_string(),
    }
}

impl Network {
    pub(super) fn new(sender: Sender<Sent>) -> Self {
        let config = ureq::Agent::config_builder().http_status_as_error(false);
        Self {
            sender,
            agent: config.build().into(),
            sending: HashSet::new(),
            listeners: HashMap::new(),
            unanswered: HashMap::new(),
            numbered: 0,
        }
    }

    fn number(&mut self) -> i64 {
        self.numbered += 1;
        self.numbered
    }

    /// Whether a thread may yet send something a task waits for.
    pub(super) fn waits(&self) -> bool {
        let accepting = |listener: &Listener| !listener.waiting.is_empty();
        !self.sending.is_empty() || self.listeners.values().any(accepting)
    }

    pub(super) fn send(&mut self, ticket: u64, request: Request) {
        self.sending.insert(ticket);
        let (agent, sender) = (self.agent.clone(), self.sender.clone());
        thread::spawn(move || {
            let _ = sender.send(Sent::Response(ticket, send(&agent, request)));
        });
    }

    /// Whether a response is still waited for, which it then is not.
    pub(super) fn sent(&mut self, ticket: u64) -> bool {
        self.sending.remove(&ticket)
    }

    pub(super) fn listen(&mut self, host: &str, port: i64) -> Result<Listening, Failure> {
        let port = u16::try_from(port).map_err(|_| failed(Kind::Invalid, &"no such port"))?;
        let server = tiny_http::Server::http((host, port)).map_err(|error| {
            let denied = error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
            failed(if denied { Kind::Denied } else { Kind::Failed }, &error)
        })?;
        let port = server
            .server_addr()
            .to_ip()
            .map_or(0, |address| address.port());
        let server = Arc::new(server);
        let id = self.number();
        let (requests, sender) = (Arc::clone(&server), self.sender.clone());
        // Ends when the server is unblocked, or the program is gone.
        thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                if sender.send(Sent::Request(id, request)).is_err() {
                    return;
                }
            }
        });
        let listener = Listener {
            server,
            unaccepted: VecDeque::new(),
            waiting: VecDeque::new(),
        };
        self.listeners.insert(id, listener);
        Ok(Listening {
            id,
            port: i64::from(port),
        })
    }

    /// Hands a request a server was sent to whatever waits to accept
    /// one, or keeps it until something does. One whose body cannot
    /// be read is refused here.
    pub(super) fn arrived(
        &mut self,
        server: i64,
        mut request: tiny_http::Request,
    ) -> Option<(u64, Done)> {
        let mut body = Vec::new();
        if request.as_reader().read_to_end(&mut body).is_err() {
            let refusal = tiny_http::Response::from_string("the body could not be read");
            let _ = request.respond(refusal.with_status_code(400));
            return None;
        }
        let header = |header: &tiny_http::Header| {
            let name = header.field.as_str().as_str().to_ascii_lowercase();
            (name, header.value.as_str().to_owned())
        };
        let incoming = Incoming {
            id: self.number(),
            request: Request {
                method: request.method().as_str().to_owned(),
                url: request.url().to_owned(),
                headers: request.headers().iter().map(header).collect(),
                body,
            },
        };
        // A request to a server that was closed meanwhile is dropped,
        // which ends its connection.
        let listener = self.listeners.get_mut(&server)?;
        self.unanswered.insert(incoming.id, request);
        let Some(ticket) = listener.waiting.pop_front() else {
            listener.unaccepted.push_back(incoming);
            return None;
        };
        Some((ticket, Done::Accepted(Some(incoming))))
    }

    /// The next request of a server, if one is there or none will be;
    /// otherwise the ticket waits for one.
    pub(super) fn accept(&mut self, ticket: u64, server: i64) -> Option<Done> {
        let Some(listener) = self.listeners.get_mut(&server) else {
            return Some(Done::Accepted(None));
        };
        let next = listener.unaccepted.pop_front();
        if next.is_none() {
            listener.waiting.push_back(ticket);
        }
        next.map(|incoming| Done::Accepted(Some(incoming)))
    }

    pub(super) fn respond(&mut self, incoming: i64, response: Response) -> Result<(), Failure> {
        let request = self
            .unanswered
            .remove(&incoming)
            .ok_or_else(|| failed(Kind::Invalid, &"the request was answered already"))?;
        let status =
            u16::try_from(response.status).map_err(|_| failed(Kind::Invalid, &"no such status"))?;
        let mut answer = tiny_http::Response::from_data(response.body).with_status_code(status);
        for (name, value) in &response.headers {
            // The length is the body's, whatever a header says.
            if name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            let header = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                .map_err(|()| failed(Kind::Invalid, &format!("`{name}` is not a header")))?;
            answer.add_header(header);
        }
        request
            .respond(answer)
            .map_err(|error| failed(Kind::Failed, &error))
    }

    /// Stops a server, and gives the tickets that waited to accept.
    pub(super) fn close(&mut self, server: i64) -> Vec<u64> {
        let Some(listener) = self.listeners.remove(&server) else {
            return Vec::new();
        };
        listener.server.unblock();
        for incoming in listener.unaccepted {
            self.unanswered.remove(&incoming.id);
        }
        listener.waiting.into()
    }

    /// Stops waiting for what a ticket waited for. A request already
    /// sent goes on, and its response is dropped.
    pub(super) fn forsake(&mut self, ticket: u64) {
        self.sending.remove(&ticket);
        for listener in self.listeners.values_mut() {
            listener.waiting.retain(|waiting| *waiting != ticket);
        }
    }
}

/// Sends a request and reads the whole response.
fn send(agent: &ureq::Agent, request: Request) -> Result<Response, Failure> {
    let invalid = |error: http::Error| failed(Kind::Invalid, &error);
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(request.url.as_str());
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(request.body).map_err(invalid)?;
    let mut response = agent.run(request).map_err(|error| refused(&error))?;
    let header = |(name, value): (&http::HeaderName, &http::HeaderValue)| {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        (name.as_str().to_owned(), value)
    };
    let headers = response.headers().iter().map(header).collect();
    let body = response.body_mut().read_to_vec();
    Ok(Response {
        status: i64::from(response.status().as_u16()),
        headers,
        body: body.map_err(|error| failed(Kind::Invalid, &error))?,
    })
}

/// Why a request got no response.
fn refused(error: &ureq::Error) -> Failure {
    use ureq::Error;
    let kind = match error {
        Error::BadUri(_) | Error::Http(_) => Kind::Invalid,
        Error::HostNotFound | Error::ConnectionFailed | Error::Timeout(_) | Error::Io(_) => {
            Kind::Unreachable
        }
        _ => Kind::Failed,
    };
    failed(kind, &error)
}
