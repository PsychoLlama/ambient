//! HTTP as a world carries it, and as `system.http` describes it: what
//! is sent and received, as values a program holds.

use std::collections::BTreeMap;

use super::{Failure, Work, World, error, int, text};
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
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub status: i64,
    pub headers: Vec<(String, String)>,
    pub body: String,
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
            ("body", Value::String(self.body)),
        ])
    }

    fn read(value: &Value) -> Result<Self, String> {
        Ok(Self {
            method: field_text(value, "method")?,
            url: field_text(value, "url")?,
            headers: field_headers(value)?,
            body: field_text(value, "body")?,
        })
    }
}

impl Response {
    pub(super) fn value(self) -> Value {
        record([
            ("status", Value::Int(self.status)),
            ("headers", headers(self.headers)),
            ("body", Value::String(self.body)),
        ])
    }

    fn read(value: &Value) -> Result<Self, String> {
        Ok(Self {
            status: field_int(value, "status")?,
            headers: field_headers(value)?,
            body: field_text(value, "body")?,
        })
    }
}

impl Incoming {
    pub(super) fn value(self) -> Value {
        record([
            ("id", Value::Int(self.id)),
            ("request", self.request.value()),
        ])
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, String> {
    let Value::Record(fields) = value else {
        return Err("not a record".to_owned());
    };
    fields.get(name).ok_or_else(|| format!("no field `{name}`"))
}

fn field_text(value: &Value, name: &str) -> Result<String, String> {
    match field(value, name)? {
        Value::String(text) => Ok(text.clone()),
        _ => Err(format!("field `{name}` is not a string")),
    }
}

fn field_int(value: &Value, name: &str) -> Result<i64, String> {
    match field(value, name)? {
        Value::Int(number) => Ok(*number),
        _ => Err(format!("field `{name}` is not an integer")),
    }
}

fn field_headers(value: &Value) -> Result<Vec<(String, String)>, String> {
    let Value::List(headers) = field(value, "headers")? else {
        return Err("field `headers` is not a list".to_owned());
    };
    let pair = |header: &Value| match header {
        Value::Tuple(pair) => match &pair[..] {
            [Value::String(name), Value::String(value)] => Ok((name.clone(), value.clone())),
            _ => Err("a header is not a name and a value".to_owned()),
        },
        _ => Err("a header is not a pair".to_owned()),
    };
    headers.iter().map(pair).collect()
}

/// What `listen`, `respond`, and `close` do: none of them waits.
pub(super) type Handler = fn(&mut dyn World, &[Value]) -> Result<Value, String>;

pub(super) const HANDLERS: [(&str, Handler); 3] = [
    ("http_listen", |world, args| {
        let listening = world.listen(text(args, 0)?, int(args, 1)?);
        Ok(listening.map_or_else(
            |failure| error("http", &failure),
            |Listening { id, port }| record([("id", Value::Int(id)), ("port", Value::Int(port))]),
        ))
    }),
    ("http_respond", |world, args| {
        let incoming = field_int(args.first().unwrap_or(&Value::Null), "id")?;
        let response = Response::read(args.get(1).unwrap_or(&Value::Null))?;
        let answered = world.respond(incoming, response);
        Ok(answered.map_or_else(|failure| error("http", &failure), |()| Value::Null))
    }),
    ("http_close", |world, args| {
        world.close(field_int(args.first().unwrap_or(&Value::Null), "id")?);
        Ok(Value::Null)
    }),
];

/// What `send` and `accept` begin.
pub(super) type Starter = fn(&[Value]) -> Result<Work, String>;

pub(super) const STARTERS: [(&str, Starter); 2] = [
    ("http_send", |args| {
        let request = Request::read(args.first().unwrap_or(&Value::Null))?;
        Ok(Work::Send(request))
    }),
    ("http_accept", |args| {
        let server = field_int(args.first().unwrap_or(&Value::Null), "id")?;
        Ok(Work::Accept(server))
    }),
];

/// Why a request got no response, as `system.http` describes it.
pub(super) fn sent(sent: Result<Response, Failure>) -> Value {
    sent.map_or_else(|failure| error("http", &failure), Response::value)
}
