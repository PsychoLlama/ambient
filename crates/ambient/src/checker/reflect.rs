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
use crate::types::{Env, Ty};
use crate::value::Value;

impl Checker<'_> {
    /// The tree of the type a comptime argument names.
    pub(super) fn type_argument(&self, arg: &Expr<Reference>) -> Result<Value, Error> {
        let no_tree = |found: String| Error::NoTree {
            symbol: self.symbol.clone(),
            found,
        };
        let Expr::Name(name @ Reference::Definition(symbol)) = arg else {
            return Err(no_tree("what is not the name of a type".to_owned()));
        };
        // A function is handed as the type it has, and a type that
        // refers to itself as its definition, which names it where it
        // does.
        let ty = match (self.env.functions.get(symbol), self.env.types.get(symbol)) {
            (Some(function), _) if function.type_params.is_empty() => Ty::Function {
                params: function.params.clone(),
                ret: Box::new(function.ret.clone()),
                row: function
                    .row
                    .iter()
                    .cloned()
                    .map(Reference::Definition)
                    .collect(),
            },
            (_, Some(info)) if info.params == 0 => {
                self.env.normalize(&Ty::Named(name.clone(), Vec::new()))
            }
            _ => return Err(no_tree(format!("`{symbol}`"))),
        };
        type_tree(self.env, &self.env.unfold(&ty)).ok_or_else(|| no_tree(self.env.show(&ty)))
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
pub(crate) fn reference(symbol: &Symbol) -> Value {
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

fn all(env: &Env, types: &[Ty]) -> Option<Value> {
    Some(Value::List(
        types
            .iter()
            .map(|ty| type_tree(env, ty))
            .collect::<Option<_>>()?,
    ))
}

/// The abilities of a row, each as a `tree.Ability`: which it is, and
/// what it takes and gives. One with type parameters has no tree yet.
fn abilities(env: &Env, row: &[Reference]) -> Option<Value> {
    let ability = |ability: &Reference| {
        let Reference::Definition(symbol) = ability else {
            return None;
        };
        let info = env
            .abilities
            .get(symbol)
            .filter(|info| info.type_params.is_empty())?;
        let params: Vec<Ty> = info
            .params
            .iter()
            .map(|param| env.normalize(param))
            .collect();
        Some(record([
            ("ability", reference(symbol)),
            ("function", all(env, &params)?),
            ("returns", type_tree(env, &env.normalize(&info.ret))?),
        ]))
    };
    Some(Value::List(row.iter().map(ability).collect::<Option<_>>()?))
}

/// A normalized type as a `tree.Type`, if it has a tree: a type with
/// arguments that refers to itself, a method, and what is held under a
/// key that is not a field have none yet.
fn type_tree(env: &Env, ty: &Ty) -> Option<Value> {
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
        Ty::List(member) => record([("list", type_tree(env, member)?)]),
        Ty::Cell(member) => record([("mutable", type_tree(env, member)?)]),
        Ty::Iterator(member) => record([("iterator", type_tree(env, member)?)]),
        Ty::Tuple(members) => record([("tuple", all(env, members)?)]),
        Ty::Union(members) => record([("union", all(env, members)?)]),
        Ty::Record(fields) => {
            let field = |(key, ty): &(Key<Reference>, Ty)| match key {
                Key::Field(name) => {
                    let name = Value::String(name.clone());
                    Some(record([("name", name), ("holds", type_tree(env, ty)?)]))
                }
                Key::Ability(_) => None,
            };
            let fields = fields.iter().map(field).collect::<Option<_>>()?;
            record([("record", Value::List(fields))])
        }
        Ty::Function { params, ret, row } => record([
            ("function", all(env, params)?),
            ("returns", type_tree(env, ret)?),
            ("takes", abilities(env, row)?),
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
        Ty::Named(Reference::Definition(symbol), args) => {
            record([("applied", reference(symbol)), ("to", all(env, args)?)])
        }
        Ty::Branded(under, brands) => {
            let brands = brands.iter().map(brand).collect::<Option<_>>()?;
            record([
                ("brands", Value::List(brands)),
                ("under", type_tree(env, under)?),
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
