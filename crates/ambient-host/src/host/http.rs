//! HTTP as a world carries it, and as `system.http` describes it: what
//! is sent and received, as values a program holds.

use std::collections::BTreeMap;

use super::trust::Trust;
use super::{Failure, PACKAGE, Work, World, error, int, text};
use crate::value::Value;

/// What is sent to a server. One a server was sent has the path and
/// query it was asked for as its `url`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    /// Each header's name, in lower case when the world gives it, and
    /// its value.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub status: i64,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The head of a response, and the stream its body arrives on, by the
/// number a world knows it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streamed {
    pub status: i64,
    pub headers: Vec<(String, String)>,
    pub body: i64,
}

impl Streamed {
    pub(super) fn value(self) -> Value {
        record([
            ("status", Value::Int(self.status)),
            ("headers", headers(self.headers)),
            ("body", handle(super::net::STREAM, self.body)),
        ])
    }
}

impl Trust {
    /// Whom a request trusts, as `system.http` describes it, or
    /// nothing for whom the world does.
    fn read(value: &Value) -> Result<Option<Self>, String> {
        let optional = |name: &str| match field(value, name)? {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(format!("field `{name}` is not a string")),
        };
        if *value == Value::Null {
            return Ok(None);
        }
        let pin = match field(value, "pin")? {
            Value::Null => None,
            Value::Bytes(digest) => Some(digest.clone()),
            _ => return Err("field `pin` is not bytes".to_owned()),
        };
        Ok(Some(Self {
            roots: optional("roots")?,
            pin,
            address: optional("address")?,
        }))
    }
}

/// A request a server was sent, and what to answer it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub id: i64,
    pub request: Request,
}

/// A listener a world holds, and the port it listens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listening {
    pub id: i64,
    pub port: i64,
}

/// What `system.http` calls the brand of a server's listener, and of
/// the way to answer a request.
pub(super) const LISTENER: &str = "http.Listener";
pub(super) const REPLY: &str = "http.Reply";

/// The handle a program holds for what a world numbered, under a
/// brand of `system.http`.
pub(super) fn handle(brand: &str, number: i64) -> Value {
    Value::Handle {
        package: PACKAGE.to_owned(),
        brand: brand.to_owned(),
        number: number.cast_unsigned(),
    }
}

/// What a world numbered that a field of a record is the handle for.
fn held(value: &Value, name: &str, brand: &str) -> Result<i64, String> {
    match field(value, name)? {
        Value::Handle {
            package,
            brand: minted,
            number,
        } if package == PACKAGE && minted == brand => Ok(number.cast_signed()),
        _ => Err(format!("field `{name}` is not a handle of `{brand}`")),
    }
}

impl Listening {
    fn value(self) -> Value {
        record([
            ("port", Value::Int(self.port)),
            ("listener", handle(LISTENER, self.id)),
        ])
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let named = |(name, value): (&str, Value)| (name.to_owned(), value);
    Value::Record(fields.into_iter().map(named).collect::<BTreeMap<_, _>>())
}

fn headers(headers: Vec<(String, String)>) -> Value {
    let pair = |(name, value)| Value::Tuple(vec![Value::String(name), Value::String(value)]);
    Value::List(headers.into_iter().map(pair).collect())
}

impl Request {
    fn value(self) -> Value {
        record([
            ("method", Value::String(self.method)),
            ("url", Value::String(self.url)),
            ("headers", headers(self.headers)),
            ("body", Value::Bytes(self.body)),
        ])
    }

    fn read(value: &Value) -> Result<Self, String> {
        Ok(Self {
            method: field_text(value, "method")?,
            url: field_text(value, "url")?,
            headers: field_headers(value)?,
            body: field_bytes(value, "body")?,
        })
    }
}

impl Response {
    pub(super) fn value(self) -> Value {
        record([
            ("status", Value::Int(self.status)),
            ("headers", headers(self.headers)),
            ("body", Value::Bytes(self.body)),
        ])
    }

    fn read(value: &Value) -> Result<Self, String> {
        Ok(Self {
            status: field_int(value, "status")?,
            headers: field_headers(value)?,
            body: field_bytes(value, "body")?,
        })
    }
}

impl Incoming {
    pub(super) fn value(self) -> Value {
        record([
            ("request", self.request.value()),
            ("reply", handle(REPLY, self.id)),
        ])
    }
}

pub(super) fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, String> {
    let Value::Record(fields) = value else {
        return Err("not a record".to_owned());
    };
    fields.get(name).ok_or_else(|| format!("no field `{name}`"))
}

pub(super) fn field_text(value: &Value, name: &str) -> Result<String, String> {
    match field(value, name)? {
        Value::String(text) => Ok(text.clone()),
        _ => Err(format!("field `{name}` is not a string")),
    }
}

pub(super) fn field_bytes(value: &Value, name: &str) -> Result<Vec<u8>, String> {
    match field(value, name)? {
        Value::Bytes(bytes) => Ok(bytes.clone()),
        _ => Err(format!("field `{name}` is not bytes")),
    }
}

fn field_int(value: &Value, name: &str) -> Result<i64, String> {
    match field(value, name)? {
        Value::Int(number) => Ok(*number),
        _ => Err(format!("field `{name}` is not an integer")),
    }
}

fn field_headers(value: &Value) -> Result<Vec<(String, String)>, String> {
    pairs(field(value, "headers")?)
}

/// A list of pairs of strings: headers, or variables.
pub(super) fn pairs(value: &Value) -> Result<Vec<(String, String)>, String> {
    let Value::List(headers) = value else {
        return Err("not a list of pairs".to_owned());
    };
    let pair = |header: &Value| match header {
        Value::Tuple(pair) => match &pair[..] {
            [Value::String(name), Value::String(value)] => Ok((name.clone(), value.clone())),
            _ => Err("a pair is not a name and a value".to_owned()),
        },
        _ => Err("a member is not a pair".to_owned()),
    };
    headers.iter().map(pair).collect()
}

/// What `listen`, `respond`, and `close` do: none of them waits.
pub(super) type Handler = fn(&mut dyn World, &[Value]) -> Result<Value, String>;

pub(super) const HANDLERS: [(&str, Handler); 3] = [
    ("http_listen", |world, args| {
        let listening = world.listen(text(args, 0)?, int(args, 1)?);
        Ok(listening.map_or_else(|failure| error("http", &failure), Listening::value))
    }),
    ("http_respond", |world, args| {
        let incoming = held(args.first().unwrap_or(&Value::Null), "reply", REPLY)?;
        let response = Response::read(args.get(1).unwrap_or(&Value::Null))?;
        let answered = world.respond(incoming, response);
        Ok(answered.map_or_else(|failure| error("http", &failure), |()| Value::Null))
    }),
    ("http_close", |world, args| {
        world.close(held(
            args.first().unwrap_or(&Value::Null),
            "listener",
            LISTENER,
        )?);
        Ok(Value::Null)
    }),
];

/// What `send` and `accept` begin.
pub(super) type Starter = fn(&[Value]) -> Result<Work, String>;

pub(super) const STARTERS: [(&str, Starter); 3] = [
    ("http_open", |args| {
        let request = Request::read(args.first().unwrap_or(&Value::Null))?;
        let trust = Trust::read(args.get(1).unwrap_or(&Value::Null))?;
        Ok(Work::Open(request, trust))
    }),
    ("http_send", |args| {
        let request = Request::read(args.first().unwrap_or(&Value::Null))?;
        Ok(Work::Send(request))
    }),
    ("http_accept", |args| {
        let server = held(args.first().unwrap_or(&Value::Null), "listener", LISTENER)?;
        Ok(Work::Accept(server))
    }),
];

/// Why a request got no response, as `system.http` describes it.
pub(super) fn sent(sent: Result<Response, Failure>) -> Value {
    sent.map_or_else(|failure| error("http", &failure), Response::value)
}
