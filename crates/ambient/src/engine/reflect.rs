//! What a build is told of a program and of the world outside it: a
//! package's modules, a module's definitions, what a definition
//! reaches, a run of a function, and the bytes at a URL. See "Build
//! abilities" in `docs/spec/tools.md`.

use std::collections::HashSet;

use super::Engine;
use super::asked::{record, refused};
use crate::checker;
use crate::error::Error;
use crate::exchange;
use crate::hash::{Hash, Hasher};
use crate::surface::GENERATOR;
use crate::symbol::{ModuleId, PackageId, Symbol};
use crate::value::Value;

/// What gives the bytes at a URL, or says why it cannot.
pub(super) type Fetch<'a> = Box<dyn Fn(&str) -> Result<Vec<u8>, String> + 'a>;

pub(super) use crate::project::digest;

/// Where the store keeps the bytes of a digest, fetched or kept.
fn held(digest: &str) -> Hash {
    Hasher::new("kept").bytes(digest.as_bytes()).finish()
}

/// The module a `build.Module` among `args` names.
fn module(args: &[Value], position: usize) -> Result<ModuleId, String> {
    let wrong = || format!("argument {position} is not a module");
    let Some(Value::Record(fields)) = args.get(position) else {
        return Err(wrong());
    };
    let (Some(Value::String(package)), Some(Value::List(path))) =
        (fields.get("package"), fields.get("path"))
    else {
        return Err(wrong());
    };
    let mut parts = Vec::new();
    for part in path {
        let Value::String(part) = part else {
            return Err(wrong());
        };
        parts.push(part.as_str());
    }
    Ok(ModuleId::new(package, &parts.join(".")))
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
            "definitions" => module(args, 0).map(|module| match self.defined(&module) {
                Ok(defined) => Value::List(defined.iter().map(checker::reference).collect()),
                Err(refused) => refused,
            }),
            "annotated" => self.annotated(args),
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
    /// as a `build.Module`.
    fn modules(&self, package: &str) -> Value {
        let path = |module: ModuleId| {
            let path = module.path.0.into_iter().map(Value::String).collect();
            record([
                ("package", Value::String(package.to_owned())),
                ("path", Value::List(path)),
            ])
        };
        let modules = self.sources.modules(&PackageId(package.to_owned()));
        Value::List(modules.into_iter().map(path).collect())
    }

    /// The definitions of a module, public and not, in the order they
    /// are written, and then those its call gives if it is generated;
    /// or the error a build is given in their place.
    fn defined(&self, module: &ModuleId) -> Result<Vec<Symbol>, Value> {
        let Some(read) = self.sources.module(module) else {
            return Err(refused("not_found", format!("no module `{module}`")));
        };
        let written = read.definitions().iter();
        let written = written
            .filter(|definition| definition.name != GENERATOR)
            .map(|definition| module.symbol(definition.kind.namespace(), &definition.name));
        let mut symbols: Vec<Symbol> = written.collect();
        let generated = self.generated_in(module);
        symbols.extend(generated.map_err(|error| refused("failed", error.to_string()))?);
        Ok(symbols)
    }

    /// What the definitions of a module are themselves annotated with
    /// under a key, as a list of `build.Annotation`. Nothing is
    /// checked for it, so it holds whatever the definitions reach.
    fn annotated(&self, args: &[Value]) -> Result<Value, String> {
        let module = module(args, 0)?;
        let Some(Value::Tag { package, tag }) = args.get(1) else {
            return Err("an annotation's key is a tag".to_owned());
        };
        let defined = match self.defined(&module) {
            Ok(defined) => defined,
            Err(refused) => return Ok(refused),
        };
        let mut found = Vec::new();
        for symbol in &defined {
            let said = match self.annotations_of(symbol) {
                Ok(said) => said,
                Err(error) => return Ok(refused("failed", error.to_string())),
            };
            let keyed = said.iter();
            let keyed = keyed.filter(|said| said.package == *package && said.tag == *tag);
            found.extend(keyed.map(|said| {
                // What was encoded here decodes.
                let value = exchange::decode(&said.value).unwrap_or(Value::Null);
                record([("on", checker::reference(symbol)), ("value", value)])
            }));
        }
        Ok(Value::List(found))
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
