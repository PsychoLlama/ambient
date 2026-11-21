//! Comptime calls: a function run during checking, and its result put in
//! the tree. See `docs/spec/comptime.md`.

use super::{Checker, reify};
use crate::core;
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Expr, Key, Reference};
use crate::symbol::{ModuleId, Symbol};
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
        let handed = self.handed(function);
        self.providing = true;
        let call_checked = if handed.iter().any(|handed| *handed != Handed::Value) {
            self.given(args, function, &handed)
        } else {
            self.infer(call).map(|_| ())
        };
        self.providing = false;
        call_checked?;
        self.frame().slots.truncate(slots);
        self.frame().lambdas.truncate(lambdas);

        // An argument is known when its type has one value, and one
        // that names a type for a parameter that takes its tree is that.
        self.next = start;
        let mut values = Vec::new();
        for (arg, handed) in args
            .iter()
            .zip(handed.iter().chain(std::iter::repeat(&Handed::Value)))
        {
            match handed {
                Handed::Type => values.push(self.type_argument(arg)?),
                Handed::Reference => values.push(self.reference_argument(arg)?),
                Handed::Value => {}
            }
            if *handed != Handed::Value {
                continue;
            }
            values.push(self.argument(arg)?);
        }
        self.frame().slots.truncate(slots);
        self.frame().lambdas.truncate(lambdas);

        let Some(evaluator) = self.evaluator else {
            return Ok((core::Expr::Null, Ty::Never));
        };
        // What the function takes, it takes with the authority of the
        // package the call is written in.
        let row = self.env.functions.get(function).map(|taken| &taken.row);
        let package = &self.symbol.module.package;
        let ungranted = |ability: &&Symbol| !evaluator.granted(package, ability);
        if let Some(ability) = row.into_iter().flatten().find(ungranted) {
            return Err(Error::Ungranted {
                symbol: self.symbol.clone(),
                ability: ability.clone(),
            });
        }
        let result = evaluator.evaluate(package, function, &values)?;
        let spliced = splice(&result).ok_or_else(|| Error::NotSpliceable {
            symbol: self.symbol.clone(),
        })?;
        // What a function gives as a tree of code is that code, and
        // anything else the value it is.
        let code = Ty::Named(Reference::Definition(reify::code()), Vec::new());
        let ret = self.env.functions.get(function).map(|taken| &taken.ret);
        if ret == Some(&self.env.normalize(&code)) {
            return self.code(&spliced.0);
        }
        Ok(spliced)
    }
}

impl Checker<'_> {
    /// The value an argument of a comptime call is known to be: what a
    /// constant it names holds, the one value its type has, or a
    /// list, a tuple, or a record written in place of such. A list's
    /// type says neither how long it is nor which member is where, so
    /// one is known only by what is written.
    fn argument(&mut self, arg: &Expr<Reference>) -> Result<Value, Error> {
        let mut all = |members: &[Expr<Reference>]| {
            let known = members.iter().map(|member| self.argument(member));
            known.collect::<Result<Vec<_>, _>>()
        };
        match arg {
            Expr::List(members) => return Ok(Value::List(all(members)?)),
            Expr::Tuple(members) => return Ok(Value::Tuple(all(members)?)),
            Expr::Record(fields) if fields.iter().all(|(key, _)| matches!(key, Key::Field(_))) => {
                let (keys, values): (Vec<_>, Vec<_>) = fields.iter().cloned().unzip();
                let named = keys.into_iter().filter_map(|key| match key {
                    Key::Field(name) => Some(name),
                    Key::Ability(_) => None,
                });
                return Ok(Value::Record(named.zip(all(&values)?).collect()));
            }
            Expr::Module(module) => return Ok(module_value(module)),
            _ => {}
        }
        let (_, ty) = self.infer(arg)?;
        let held = match (arg, self.evaluator) {
            (Expr::Name(Reference::Definition(named)), Some(evaluator))
                if self.env.constants.contains_key(named) =>
            {
                evaluator.constant(named)?
            }
            _ => None,
        };
        held.or_else(|| value_of(&ty))
            .ok_or_else(|| Error::NotComptime {
                symbol: self.symbol.clone(),
                found: self.env.show(&ty),
            })
    }

    /// Whether whatever a comptime call gives is within `expected`,
    /// by what its function is declared to give alone.
    ///
    /// A value is of the type its function gives, so the literal it
    /// is spliced as need not be held to `expected` part by part:
    /// that is a proof as large as the value, and a tree of code a
    /// call gives is large.
    pub(super) fn gives_within(&self, call: &Expr<Reference>, expected: &Ty) -> bool {
        let Expr::Call { callee, .. } = call else {
            return false;
        };
        let Expr::Name(Reference::Definition(function)) = &**callee else {
            return false;
        };
        let code = Ty::Named(Reference::Definition(reify::code()), Vec::new());
        self.env.functions.get(function).is_some_and(|taken| {
            // What is spliced as code has the type of the code, and a
            // generic function's result is not known from here.
            taken.type_params.is_empty()
                && taken.ret != self.env.normalize(&code)
                && self.env.subtype(&taken.ret, expected)
        })
    }
}

/// What a comptime function is handed for an argument.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Handed {
    /// The one value the argument's type has.
    Value,
    /// The tree of the type the argument names.
    Type,
    /// The reference of the definition the argument names.
    Reference,
}

impl Checker<'_> {
    /// What each parameter of `function` is handed, which its type says.
    fn handed(&self, function: &Symbol) -> Vec<Handed> {
        let named = |symbol| {
            self.env
                .normalize(&Ty::Named(Reference::Definition(symbol), Vec::new()))
        };
        let (tree, reference) = (named(reify::tree()), named(reify::reference()));
        let params = self.env.functions.get(function).map(|taken| &taken.params);
        let handed = |param: &Ty| match self.env.normalize(param) {
            param if param == tree => Handed::Type,
            param if param == reference => Handed::Reference,
            _ => Handed::Value,
        };
        params.into_iter().flatten().map(handed).collect()
    }

    /// Checks the arguments of a call that names types: each of the
    /// others against the parameter it is for.
    fn given(
        &mut self,
        args: &[Expr<Reference>],
        function: &Symbol,
        handed: &[Handed],
    ) -> Result<(), Error> {
        let params = self
            .env
            .functions
            .get(function)
            .map(|taken| taken.params.clone());
        let params = params.unwrap_or_default();
        if params.len() != args.len() {
            return Err(Error::Arity {
                symbol: self.symbol.clone(),
                callee: function.clone(),
                expected: params.len(),
                found: args.len(),
            });
        }
        for ((arg, param), handed) in args.iter().zip(&params).zip(handed) {
            if *handed == Handed::Value {
                self.check(arg, param)?;
            }
        }
        Ok(())
    }
}

/// A module as a `build.Module`: the package it is of, named as a
/// reference names it, and its path.
fn module_value(module: &ModuleId) -> Value {
    let path = module.path.0.iter().cloned().map(Value::String);
    Value::Record(
        [
            (
                "package".to_owned(),
                Value::String(module.package.0.clone()),
            ),
            ("path".to_owned(), Value::List(path.collect())),
        ]
        .into(),
    )
}

/// A module a comptime argument names, as the data it is handed as.
pub(super) fn module(module: &ModuleId) -> (core::Expr, Ty) {
    // Text and a list of text are spliced whatever they hold.
    splice(&module_value(module)).unwrap_or((core::Expr::Null, Ty::Never))
}

/// The one value a singleton type has.
fn value_of(ty: &Ty) -> Option<Value> {
    Some(match ty {
        Ty::Null => Value::Null,
        Ty::Literal(literal) => literal.value(),
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
        Value::Bytes(value) => literal(Literal::Bytes(value.clone())),
        // A handle is its host's, and a build has none to give a program.
        Value::Handle { .. } => return None,
        Value::Tuple(members) => {
            let (exprs, types) = all(members)?;
            (core::Expr::Tuple(exprs), Ty::Tuple(types.into()))
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
        Value::Function(_)
        | Value::Method(_)
        | Value::Cell
        | Value::Iterator(..)
        | Value::Token
        | Value::Types => {
            return None;
        }
    })
}
