//! Annotations: what a definition says to a build, and what a build
//! is told of them and of the definitions they are on. See
//! `spec/annotations.md` and "Build abilities" in `spec/tools.md`.

use std::collections::{HashMap, HashSet};

use super::Engine;
use super::asked::record;
use crate::checker;
use crate::core::{Annotation, Tree};
use crate::error::Error;
use crate::exchange;
use crate::hash::Hasher;
use crate::literal::Literal;
use crate::surface::{DefinitionKind, Expr, Key, Reference, Type};
use crate::symbol::Symbol;
use crate::types::Ty;
use crate::value::Value;

/// The value an expression of plain data is, if it is one: a literal,
/// or a list, a tuple, or a record of them.
fn data(expr: &Expr<Reference>) -> Option<Value> {
    let all = |members: &[Expr<Reference>]| members.iter().map(data).collect::<Option<Vec<_>>>();
    Some(match expr {
        Expr::Null => Value::Null,
        Expr::Literal(Literal::Bool(value)) => Value::Bool(*value),
        Expr::Literal(Literal::Int(value)) => Value::Int(*value),
        Expr::Literal(Literal::Float(bits)) => Value::Float(f64::from_bits(*bits)),
        Expr::Literal(Literal::String(value)) => Value::String(value.clone()),
        Expr::Literal(Literal::Bytes(value)) => Value::Bytes(value.clone()),
        Expr::Literal(Literal::Tag { package, tag }) => Value::Tag {
            package: package.clone(),
            tag: tag.clone(),
        },
        Expr::List(members) => Value::List(all(members)?),
        Expr::Tuple(members) => Value::Tuple(all(members)?),
        Expr::Record(fields) => Value::Record(
            fields
                .iter()
                .map(|(key, value)| match key {
                    Key::Field(name) => Some((name.clone(), data(value)?)),
                    Key::Ability(_) => None,
                })
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

impl Engine<'_> {
    /// What `symbol` is annotated with, in the order it is written.
    ///
    /// # Errors
    ///
    /// Fails if a key is no tag, or a value is not plain data.
    pub(super) fn annotations_of(&self, symbol: &Symbol) -> Result<Vec<Annotation>, Error> {
        if self.root_of(symbol).is_some() {
            return Ok(Vec::new());
        }
        let wrong = |key| Error::NotAnnotation {
            symbol: symbol.clone(),
            key,
        };
        let resolved = self.resolve(symbol)?;
        let mut annotations = Vec::new();
        for annotation in &resolved.definition.annotations {
            let Reference::Definition(key) = &annotation.key else {
                return Err(wrong(true));
            };
            // A tag is, once resolved, the type of its one value.
            let DefinitionKind::Type(tag) = &self.resolve(key)?.definition.kind else {
                return Err(wrong(true));
            };
            let Type::Literal(Literal::Tag { package, tag }) = &tag.ty else {
                return Err(wrong(true));
            };
            let value = data(&annotation.value)
                .and_then(|value| exchange::encode(&value).ok())
                .ok_or_else(|| wrong(false))?;
            annotations.push(Annotation {
                package: package.clone(),
                tag: tag.clone(),
                value,
            });
        }
        Ok(annotations)
    }

    /// The annotations of the key `package` and `tag` that `from`
    /// reaches, as a list of `build.Annotation`: in the order a walk
    /// from it first meets them, a definition met twice giving its
    /// own once. See "Collecting" in `spec/annotations.md`.
    pub(super) fn annotations(
        &self,
        from: &Symbol,
        package: &str,
        tag: &str,
    ) -> Result<Value, Error> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        // What is left to walk, the next to visit last.
        let mut pending = vec![from.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            let open = self.check(&symbol)?;
            for annotation in &open.annotations {
                if annotation.package != package || annotation.tag != tag {
                    continue;
                }
                // What was encoded here decodes.
                let value = exchange::decode(&annotation.value).unwrap_or(Value::Null);
                found.push(record([
                    ("on", checker::reference(&symbol)),
                    ("value", value),
                ]));
            }
            pending.extend(open.slots.iter().rev().cloned());
        }
        Ok(Value::List(found))
    }

    /// The type a function or an extern has, as a `tree.Type`: what it
    /// takes, what it gives, and its row.
    ///
    /// # Errors
    ///
    /// Fails if `symbol` is neither, or its type has no tree, as that
    /// of a function with type parameters has none.
    pub(super) fn signature_tree(&self, symbol: &Symbol) -> Result<Value, Error> {
        let no_tree = || Error::NoTree {
            symbol: symbol.clone(),
            found: format!("`{symbol}`"),
        };
        let (signature, named) = self.signature(symbol)?.ok_or_else(no_tree)?;
        if !signature.type_params.is_empty() {
            return Err(no_tree());
        }
        let env = self.environment(named, &[], &mut Hasher::new("signature"))?;
        let ty = Ty::Function {
            params: signature
                .params
                .iter()
                .map(|ty| env.normalize(ty))
                .collect(),
            ret: Box::new(env.normalize(&signature.ret)),
            row: signature
                .row
                .into_iter()
                .map(Reference::Definition)
                .collect(),
        };
        checker::type_tree(&env, &ty).ok_or_else(no_tree)
    }

    /// The externs `root` reaches, by the package and path native code
    /// is found by.
    pub(super) fn externs(
        &self,
        root: &Symbol,
    ) -> Result<HashMap<(String, String), Symbol>, Error> {
        let mut found = HashMap::new();
        let mut seen = HashSet::new();
        let mut pending = vec![root.clone()];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol.clone()) {
                continue;
            }
            let open = self.check(&symbol)?;
            if let Tree::Extern { package, path, .. } = &open.tree {
                found.insert((package.clone(), path.clone()), symbol);
            }
            pending.extend(open.slots.iter().cloned());
        }
        Ok(found)
    }
}
