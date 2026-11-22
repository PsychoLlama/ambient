//! HTTP for a real process: requests sent on threads of their own, and
//! servers whose requests a thread each brings to the program.
//!
//! Only the program's thread answers a request or hands one over, so
//! what is here is the same kind of waiting as the rest of
//! [`super::pending`]: a thread blocks, and sends what it got.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use ureq::http;

use super::linked::Http;
use super::pending::Sent;
use super::pinned;
use super::trust::{self, Trust};
use super::waits::Done;
use super::{Failure, Incoming, Kind, Listening, Request, Response};

/// A server, the requests it was sent that nothing has accepted, and
/// the tickets waiting to accept one, each in the order it came.
struct Listener {
    server: Arc<tiny_http::Server>,
    unaccepted: VecDeque<Incoming>,
    waiting: VecDeque<u64>,
    /// How many requests its thread has brought that nothing has
    /// accepted: the thread refuses one more than [`HELD`].
    held: Arc<AtomicUsize>,
}

/// The most requests a server holds that nothing has accepted,
/// wherever they are on the way to the program. A program that falls
/// further behind than this is not catching up.
const HELD: usize = 256;

pub(super) struct Network {
    sender: Sender<Sent>,
    /// What sends requests, and keeps connections to send more on.
    agent: ureq::Agent,
    /// What sends the requests that trust other than the world does,
    /// each kept to send more that trust the same.
    trusting: HashMap<Trust, ureq::Agent>,
    /// The tickets of the requests that have no response yet.
    sending: HashSet<u64>,
    listeners: HashMap<i64, Listener>,
    /// The requests servers accepted that have not been answered.
    unanswered: HashMap<i64, tiny_http::Request>,
    /// How many servers and requests have been numbered.
    numbered: i64,
    /// The requests the servers' threads brought, each with its
    /// server, and what they are brought through.
    brought: Receiver<(i64, tiny_http::Request)>,
    bringing: Sender<(i64, tiny_http::Request)>,
}

fn failed(kind: Kind, message: &dyn ToString) -> Failure {
    Failure {
        kind,
        message: message.to_string(),
    }
}

impl Network {
    pub(super) fn new(sender: Sender<Sent>) -> Self {
        let (bringing, brought) = channel();
        Self {
            brought,
            bringing,
            sender,
            agent: trust::config(ureq::tls::TlsConfig::default()).into(),
            trusting: HashMap::new(),
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

    /// What sends the requests that trust so, made if none has yet.
    fn trusted_by(&mut self, trusted: Trust) -> Result<ureq::Agent, Failure> {
        if let Some(agent) = self.trusting.get(&trusted) {
            return Ok(agent.clone());
        }
        let agent = trust::agent(&trusted)?;
        self.trusting.insert(trusted, agent.clone());
        Ok(agent)
    }
}

impl Http for Network {
    fn waits(&self) -> bool {
        let accepting = |listener: &Listener| !listener.waiting.is_empty();
        !self.sending.is_empty() || self.listeners.values().any(accepting)
    }

    fn send(&mut self, ticket: u64, request: Request) {
        self.sending.insert(ticket);
        let (agent, sender) = (self.agent.clone(), self.sender.clone());
        thread::spawn(move || {
            let _ = sender.send(Sent::Response(ticket, send(&agent, request)));
        });
    }

    fn open(
        &mut self,
        ticket: u64,
        request: Request,
        trusted: Option<Trust>,
    ) -> Result<(), Failure> {
        let agent = match trusted {
            None => self.agent.clone(),
            Some(trusted) => self.trusted_by(trusted)?,
        };
        self.sending.insert(ticket);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let opened = begin(&agent, request).map(|response| {
                let (status, headers) = head(&response);
                let body = response.into_body().into_with_config().limit(u64::MAX);
                let body: Box<dyn Read + Send> = Box::new(body.reader());
                (status, headers, body)
            });
            let _ = sender.send(Sent::Opened(ticket, opened));
        });
        Ok(())
    }

    fn certificate(&mut self, ticket: u64, url: String, address: Option<String>) {
        self.sending.insert(ticket);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let shown = pinned::certificate(&url, address.as_deref());
            let _ = sender.send(Sent::Shown(ticket, shown));
        });
    }

    fn sent(&mut self, ticket: u64) -> bool {
        self.sending.remove(&ticket)
    }

    fn listen(&mut self, host: &str, port: i64) -> Result<Listening, Failure> {
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
        let bringing = self.bringing.clone();
        let held = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&held);
        // Ends when the server is unblocked, or the program is gone.
        thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                if count.load(Ordering::Relaxed) >= HELD {
                    let refusal = tiny_http::Response::from_string("the server is busy");
                    let _ = request.respond(refusal.with_status_code(503));
                    continue;
                }
                count.fetch_add(1, Ordering::Relaxed);
                // The request stays here, and the program is only woken:
                // what waits on every thread knows nothing of HTTP.
                if bringing.send((id, request)).is_err() || sender.send(Sent::Arrived).is_err() {
                    return;
                }
            }
        });
        let listener = Listener {
            server,
            unaccepted: VecDeque::new(),
            waiting: VecDeque::new(),
            held,
        };
        self.listeners.insert(id, listener);
        Ok(Listening {
            id,
            port: i64::from(port),
        })
    }

    fn arrived(&mut self) -> Option<(u64, Done)> {
        let (server, mut request) = self.brought.try_recv().ok()?;
        // A request to a server that was closed meanwhile is dropped,
        // which ends its connection.
        let held = Arc::clone(&self.listeners.get(&server)?.held);
        let mut body = Vec::new();
        if request.as_reader().read_to_end(&mut body).is_err() {
            held.fetch_sub(1, Ordering::Relaxed);
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
        let listener = self.listeners.get_mut(&server)?;
        self.unanswered.insert(incoming.id, request);
        let Some(ticket) = listener.waiting.pop_front() else {
            listener.unaccepted.push_back(incoming);
            return None;
        };
        held.fetch_sub(1, Ordering::Relaxed);
        Some((ticket, Done::Accepted(Some(incoming))))
    }

    fn accept(&mut self, ticket: u64, server: i64) -> Option<Done> {
        let Some(listener) = self.listeners.get_mut(&server) else {
            return Some(Done::Accepted(None));
        };
        let next = listener.unaccepted.pop_front();
        if next.is_some() {
            listener.held.fetch_sub(1, Ordering::Relaxed);
        } else {
            listener.waiting.push_back(ticket);
        }
        next.map(|incoming| Done::Accepted(Some(incoming)))
    }

    fn respond(&mut self, incoming: i64, response: Response) -> Result<(), Failure> {
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

    fn abandon(&mut self, incoming: i64) {
        self.unanswered.remove(&incoming);
    }

    fn close(&mut self, server: i64) -> Vec<u64> {
        let Some(listener) = self.listeners.remove(&server) else {
            return Vec::new();
        };
        listener.server.unblock();
        for incoming in listener.unaccepted {
            self.unanswered.remove(&incoming.id);
        }
        listener.waiting.into()
    }

    fn forsake(&mut self, ticket: u64) {
        self.sending.remove(&ticket);
        for listener in self.listeners.values_mut() {
            listener.waiting.retain(|waiting| *waiting != ticket);
        }
    }
}

/// The head of a response: its status, and its headers. A header that
/// is not text is not among those the program reads.
type Head = (i64, Vec<(String, String)>);

fn head(response: &http::Response<ureq::Body>) -> Head {
    let header = |(name, value): (&http::HeaderName, &http::HeaderValue)| {
        let value = std::str::from_utf8(value.as_bytes()).ok()?;
        Some((name.as_str().to_owned(), value.to_owned()))
    };
    let headers = response.headers().iter().filter_map(header).collect();
    (i64::from(response.status().as_u16()), headers)
}

/// Sends a request, and gives the response once its head is in.
fn begin(agent: &ureq::Agent, request: Request) -> Result<http::Response<ureq::Body>, Failure> {
    let invalid = |error: http::Error| failed(Kind::Invalid, &error);
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(request.url.as_str());
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(request.body).map_err(invalid)?;
    agent.run(request).map_err(|error| refused(&error))
}

/// Sends a request and reads the whole response.
fn send(agent: &ureq::Agent, request: Request) -> Result<Response, Failure> {
    let mut response = begin(agent, request)?;
    let (status, headers) = head(&response);
    let body = response.body_mut().read_to_vec();
    Ok(Response {
        status,
        headers,
        body: body.map_err(|error| failed(Kind::Invalid, &error))?,
    })
}

/// Why a request got no response.
fn refused(error: &ureq::Error) -> Failure {
    use ureq::Error;
    if let Error::Io(error) = error
        && pinned::unpinned(error)
    {
        return failed(Kind::Untrusted, &pinned::UNPINNED);
    }
    let kind = match error {
        Error::BadUri(_) | Error::Http(_) => Kind::Invalid,
        Error::Tls(_) | Error::Rustls(_) => Kind::Untrusted,
        // How a certificate that was not accepted arrives: what the
        // server showed is not what was to be read.
        Error::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => Kind::Untrusted,
        Error::HostNotFound | Error::ConnectionFailed | Error::Timeout(_) | Error::Io(_) => {
            Kind::Unreachable
        }
        _ => Kind::Failed,
    };
    failed(kind, &error)
}
