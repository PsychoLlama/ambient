//! Comptime calls: a function run during checking, and its result put in
//! the tree. See `spec/comptime.md`.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Expr, Key, Reference};
use crate::types::Ty;
use crate::value::Value;

impl Checker<'_> {
    /// A comptime call: the function is run now, and its result stands in
    /// for the call.
    pub(super) fn comptime(&mut self, call: &Expr<Reference>) -> Result<(core::Expr, Ty), Error> {
        let Expr::Call { callee, args } = call else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let Expr::Name(Reference::Definition(function)) = &**callee else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        // The call is checked like any other, for its argument types. What
        // it would lower to is dropped, with whatever it added.
        let slots = self.frame().slots.len();
        let lambdas = self.frame().lambdas.len();
        let start = self.next;
        // What the callee takes comes from the build target, not from
        // where the call is written.
        self.providing = true;
        let call_checked = self.infer(call);
        self.providing = false;
        call_checked?;
        self.frame().slots.truncate(slots);
        self.frame().lambdas.truncate(lambdas);

        // An argument is known when its type has one value.
        self.next = start;
        let mut values = Vec::new();
        for arg in args {
            let (_, ty) = self.infer(arg)?;
            values.push(value_of(&ty).ok_or_else(|| Error::NotComptime {
                symbol: self.symbol.clone(),
                found: self.env.show(&ty),
            })?);
        }
        self.frame().slots.truncate(slots);
        self.frame().lambdas.truncate(lambdas);

        let Some(evaluator) = self.evaluator else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        let result = evaluator.evaluate(function, &values)?;
        splice(&result).ok_or_else(|| Error::NotSpliceable {
            symbol: self.symbol.clone(),
        })
    }
}

/// The one value a singleton type has.
fn value_of(ty: &Ty) -> Option<Value> {
    Some(match ty {
        Ty::Null => Value::Null,
        Ty::Literal(Literal::Bool(value)) => Value::Bool(*value),
        Ty::Literal(Literal::Int(value)) => Value::Int(*value),
        Ty::Literal(Literal::Float(bits)) => Value::Float(f64::from_bits(*bits)),
        Ty::Literal(Literal::String(value)) => Value::String(value.clone()),
        Ty::Literal(Literal::Tag { package, tag }) => Value::Tag {
            package: package.clone(),
            tag: tag.clone(),
        },
        Ty::Tuple(members) => Value::Tuple(members.iter().map(value_of).collect::<Option<_>>()?),
        Ty::Record(fields) => Value::Record(
            fields
                .iter()
                .map(|(key, ty)| match key {
                    Key::Field(name) => Some((name.clone(), value_of(ty)?)),
                    Key::Ability(_) => None,
                })
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

/// A value as the literal that makes it, with the literal's type.
fn splice(value: &Value) -> Option<(core::Expr, Ty)> {
    let literal = |literal: Literal| (core::Expr::Literal(literal.clone()), Ty::Literal(literal));
    let all = |values: &[Value]| -> Option<(Vec<core::Expr>, Vec<Ty>)> {
        Some(
            values
                .iter()
                .map(splice)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .unzip(),
        )
    };
    Some(match value {
        Value::Null => (core::Expr::Null, Ty::Null),
        Value::Bool(value) => literal(Literal::Bool(*value)),
        Value::Int(value) => literal(Literal::Int(*value)),
        Value::Float(value) => literal(Literal::float(*value)),
        Value::String(value) => literal(Literal::String(value.clone())),
        Value::Tag { package, tag } => literal(Literal::Tag {
            package: package.clone(),
            tag: tag.clone(),
        }),
        Value::Tuple(members) => {
            let (exprs, types) = all(members)?;
            (core::Expr::Tuple(exprs), Ty::Tuple(types))
        }
        Value::List(members) => {
            let (exprs, mut types) = all(members)?;
            // A list type is not a singleton: it is a list of whatever
            // the members are.
            types.sort();
            types.dedup();
            let member = match types.len() {
                0 => Ty::Never,
                1 => types.remove(0),
                _ => Ty::Union(types),
            };
            (core::Expr::List(exprs), Ty::List(Box::new(member)))
        }
        Value::Record(fields) => {
            let mut exprs = Vec::new();
            let mut types = Vec::new();
            for (name, value) in fields {
                let (expr, ty) = splice(value)?;
                exprs.push((core::Key::Field(name.clone()), expr));
                types.push((Key::Field(name.clone()), ty));
            }
            (core::Expr::Record(exprs), Ty::Record(types))
        }
        Value::Function(_) | Value::Cell | Value::Token => return None,
    })
}
