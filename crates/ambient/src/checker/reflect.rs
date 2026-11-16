//! Types as values: what a comptime function is handed for a type its
//! call names. See "Trees as values" in `spec/comptime.md`.
//!
//! The shape is the library's `tree.Type`, which this makes values of.

use std::collections::BTreeMap;

use ambient_runtime::FAULT_PACKAGE;

use super::Checker;
use super::reify::MODULE;
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Expr, Key, Reference};
use crate::symbol::{Namespace, Symbol};
use crate::types::Ty;
use crate::value::Value;

impl Checker<'_> {
    /// The tree of the type a comptime argument names.
    pub(super) fn type_argument(&self, arg: &Expr<Reference>) -> Result<Value, Error> {
        let no_tree = |found: String| Error::NoTree {
            symbol: self.symbol.clone(),
            found,
        };
        let named = match arg {
            Expr::Name(name @ Reference::Definition(symbol)) => {
                let closed = |info: &crate::types::TypeInfo| info.params == 0;
                self.env
                    .types
                    .get(symbol)
                    .filter(|info| closed(info))
                    .map(|_| name)
            }
            _ => None,
        };
        let Some(name) = named else {
            return Err(no_tree("what is not the name of a type".to_owned()));
        };
        // A type that refers to itself is its definition, which names
        // it where it does.
        let ty = self.env.normalize(&Ty::Named(name.clone(), Vec::new()));
        type_tree(&self.env.unfold(&ty)).ok_or_else(|| no_tree(self.env.show(&ty)))
    }

    /// The reference of the definition a comptime argument names.
    pub(super) fn reference_argument(&self, arg: &Expr<Reference>) -> Result<Value, Error> {
        match arg {
            Expr::Name(Reference::Definition(symbol)) => Ok(reference(symbol)),
            _ => Err(Error::NoTree {
                symbol: self.symbol.clone(),
                found: "what is not the name of a definition".to_owned(),
            }),
        }
    }
}

/// A definition as a `tree.Reference`.
fn reference(symbol: &Symbol) -> Value {
    let text = |text: &String| Value::String(text.clone());
    let kind = match symbol.namespace {
        Namespace::Type => "type",
        Namespace::Value => "value",
        Namespace::Ability => "ability",
        Namespace::Key => "key",
    };
    record([
        ("package", text(&symbol.module.package.0)),
        (
            "module",
            Value::List(symbol.module.path.0.iter().map(text).collect()),
        ),
        ("name", text(&symbol.name)),
        ("kind", Value::String(kind.to_owned())),
    ])
}

/// A brand as a `tree.Brand`.
fn brand(ty: &Ty) -> Option<Value> {
    let Ty::Brand(brand) = ty else {
        return None;
    };
    Some(record([
        ("package", Value::String(brand.package.clone())),
        ("identity", Value::String(brand.brand.clone())),
    ]))
}

fn tag(name: &str) -> Value {
    Value::Tag {
        package: FAULT_PACKAGE.to_owned(),
        tag: format!("{MODULE}.{name}"),
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let named = fields
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value));
    Value::Record(named.collect::<BTreeMap<_, _>>())
}

fn all(types: &[Ty]) -> Option<Value> {
    Some(Value::List(
        types.iter().map(type_tree).collect::<Option<_>>()?,
    ))
}

/// The abilities of a row, each as a `tree.Reference`.
fn abilities(row: &[Reference]) -> Option<Value> {
    let ability = |ability: &Reference| match ability {
        Reference::Definition(symbol) => Some(reference(symbol)),
        Reference::Local(_) | Reference::Parameter(_) => None,
    };
    Some(Value::List(row.iter().map(ability).collect::<Option<_>>()?))
}

/// A normalized type as a `tree.Type`, if it has a tree: a type with
/// arguments that refers to itself, a method, and what is held under a
/// key that is not a field have none yet.
fn type_tree(ty: &Ty) -> Option<Value> {
    Some(match ty {
        Ty::Never => tag("none"),
        Ty::Unknown => tag("any"),
        Ty::Null => tag("nil"),
        Ty::Bool => tag("boolean"),
        Ty::Int => tag("integer"),
        Ty::Float => tag("number"),
        Ty::String => tag("text"),
        Ty::Bytes => tag("binary"),
        Ty::Tag => tag("tagged"),
        Ty::Literal(literal) => record([("literal", literal_value(literal))]),
        Ty::List(member) => record([("list", type_tree(member)?)]),
        Ty::Cell(member) => record([("mutable", type_tree(member)?)]),
        Ty::Iterator(member) => record([("iterator", type_tree(member)?)]),
        Ty::Tuple(members) => record([("tuple", all(members)?)]),
        Ty::Union(members) => record([("union", all(members)?)]),
        Ty::Record(fields) => {
            let field = |(key, ty): &(Key<Reference>, Ty)| match key {
                Key::Field(name) => {
                    let name = Value::String(name.clone());
                    Some(record([("name", name), ("holds", type_tree(ty)?)]))
                }
                Key::Ability(_) => None,
            };
            let fields = fields.iter().map(field).collect::<Option<_>>()?;
            record([("record", Value::List(fields))])
        }
        Ty::Function { params, ret, row } => record([
            ("function", all(params)?),
            ("returns", type_tree(ret)?),
            ("takes", abilities(row)?),
        ]),
        Ty::Brand(_) => record([
            ("brands", Value::List(vec![brand(ty)?])),
            ("under", Value::Null),
        ]),
        // What is left named in a normal form is a type that refers to
        // itself.
        Ty::Named(Reference::Definition(symbol), args) if args.is_empty() => {
            record([("named", reference(symbol))])
        }
        Ty::Branded(under, brands) => {
            let brands = brands.iter().map(brand).collect::<Option<_>>()?;
            record([
                ("brands", Value::List(brands)),
                ("under", type_tree(under)?),
            ])
        }
        Ty::Named(..) | Ty::Bound(_) | Ty::Generic { .. } | Ty::Method { .. } => return None,
    })
}

fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Int(value) => Value::Int(*value),
        Literal::Float(bits) => Value::Float(f64::from_bits(*bits)),
        Literal::String(value) => Value::String(value.clone()),
        Literal::Bytes(value) => Value::Bytes(value.clone()),
        Literal::Tag { package, tag } => Value::Tag {
            package: package.clone(),
            tag: tag.clone(),
        },
    }
}
