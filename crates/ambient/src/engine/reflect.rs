//! What a build is told of a program and of the world outside it: a
//! package's modules, a module's definitions, what a definition
//! reaches, a run of a function, and the bytes at a URL. See "Build
//! abilities" in `spec/tools.md`.

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use super::Engine;
use super::asked::{record, refused};
use crate::checker;
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::GENERATOR;
use crate::symbol::{ModuleId, PackageId, Symbol};
use crate::value::Value;

/// What gives the bytes at a URL, or says why it cannot.
pub(super) type Fetch<'a> = Box<dyn Fn(&str) -> Result<Vec<u8>, String> + 'a>;

/// The SHA-256 of `bytes`, as a digest is written: in hexadecimal.
pub(super) fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            // Writing to a string does not fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Where the store keeps the bytes of a digest, fetched or kept.
fn held(digest: &str) -> Hash {
    Hasher::new("kept").bytes(digest.as_bytes()).finish()
}

fn text(args: &[Value], position: usize) -> Result<&str, String> {
    match args.get(position) {
        Some(Value::String(text)) => Ok(text),
        _ => Err(format!("argument {position} is not text")),
    }
}

impl<'a> Engine<'a> {
    /// Lets the build fetch: `fetch` gives the bytes at a URL. What it
    /// gives is held to the digest a build names, and kept.
    #[must_use]
    pub fn with_fetch(mut self, fetch: impl Fn(&str) -> Result<Vec<u8>, String> + 'a) -> Self {
        self.fetch = Some(Box::new(fetch));
        self
    }

    /// The answer to one of the questions this module answers, if
    /// `question` is one.
    pub(super) fn told(&self, question: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let failed = |error: Error| refused("failed", error.to_string());
        Some(match question {
            "modules" => text(args, 0).map(|package| self.modules(package)),
            "definitions" => self.listed(args),
            "doc" => super::asked::definition(args, 0).map(|of| match self.resolve(&of) {
                Ok(resolved) => resolved
                    .definition
                    .doc
                    .clone()
                    .map_or(Value::Null, Value::String),
                Err(error) => failed(error),
            }),
            "reach" => super::asked::definition(args, 0)
                .map(|from| self.reaches(&from).unwrap_or_else(failed)),
            "run" => super::asked::definition(args, 0).map(|function| {
                let given = match args.get(1) {
                    Some(Value::List(given)) => given.as_slice(),
                    _ => &[],
                };
                match self.tool(&function, given) {
                    Ok(gave) => record([("gave", gave)]),
                    Err(error) => failed(error),
                }
            }),
            "keep" => match args.first() {
                Some(Value::Bytes(bytes)) => {
                    let named = digest(bytes);
                    self.store.put(held(&named), bytes.clone());
                    Ok(Value::String(named))
                }
                _ => Err("argument 0 is not bytes".to_owned()),
            },
            "kept" => text(args, 0).map(|named| match self.store.get(&held(named)) {
                Some(kept) => Value::Bytes(kept.to_vec()),
                None => refused("not_found", format!("nothing is kept under {named}")),
            }),
            "fetch" => text(args, 0).and_then(|url| Ok(self.fetched(url, text(args, 1)?))),
            _ => return None,
        })
    }

    /// The modules of the package a workspace calls `package`, each
    /// as its path.
    fn modules(&self, package: &str) -> Value {
        let path =
            |module: ModuleId| Value::List(module.path.0.into_iter().map(Value::String).collect());
        let modules = self.sources.modules(&PackageId(package.to_owned()));
        Value::List(modules.into_iter().map(path).collect())
    }

    /// The definitions of a module, public and not, in the order they
    /// are written, and then those its call gives if it is generated.
    fn listed(&self, args: &[Value]) -> Result<Value, String> {
        let Some(Value::List(path)) = args.get(1) else {
            return Err("argument 1 is not a module's path".to_owned());
        };
        let mut parts = Vec::new();
        for part in path {
            let Value::String(part) = part else {
                return Err("argument 1 is not a module's path".to_owned());
            };
            parts.push(part.as_str());
        }
        let module = ModuleId::new(text(args, 0)?, &parts.join("."));
        let Some(read) = self.sources.module(&module) else {
            return Ok(refused("not_found", format!("no module `{module}`")));
        };
        let written = read.definitions().iter();
        let written = written
            .filter(|definition| definition.name != GENERATOR)
            .map(|definition| module.symbol(definition.kind.namespace(), &definition.name));
        let mut symbols: Vec<Symbol> = written.collect();
        match self.generated_in(&module) {
            Ok(generated) => symbols.extend(generated),
            Err(error) => return Ok(refused("failed", error.to_string())),
        }
        Ok(Value::List(
            symbols.iter().map(checker::reference).collect(),
        ))
    }

    /// The definitions `from` reaches, itself first, in the order a
    /// walk from it first meets them. A lambda is part of the
    /// definition it is written in.
    fn reaches(&self, from: &Symbol) -> Result<Value, Error> {
        let mut reached = Vec::new();
        let mut seen = HashSet::new();
        self.walk(from, &mut |symbol, _| {
            if symbol.lambda.is_empty() && seen.insert(symbol.clone()) {
                reached.push(checker::reference(symbol));
            }
        })?;
        Ok(Value::List(reached))
    }

    /// The bytes at `url`, which are those of `digest`: from what was
    /// kept when they were last fetched, or else fetched now and kept.
    fn fetched(&self, url: &str, wanted: &str) -> Value {
        let key = held(wanted);
        if let Some(kept) = self.store.get(&key) {
            return Value::Bytes(kept.to_vec());
        }
        let Some(fetch) = &self.fetch else {
            return refused("denied", format!("{url}: this build fetches nothing"));
        };
        match fetch(url) {
            Ok(bytes) if digest(&bytes) == wanted => {
                self.store.put(key, bytes.clone());
                Value::Bytes(bytes)
            }
            Ok(bytes) => refused(
                "failed",
                format!("{url}: its digest is {}, not {wanted}", digest(&bytes)),
            ),
            Err(why) => refused("failed", format!("{url}: {why}")),
        }
    }
}
