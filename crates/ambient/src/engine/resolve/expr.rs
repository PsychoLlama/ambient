//! Resolving patterns and expressions: the part of a definition that
//! binds names.

use super::Resolver;
use crate::error::Error;
use crate::surface::{Arm, Expr, Key, Pattern, Reference, TypeParam};
use crate::symbol::Namespace;

impl Resolver<'_> {
    /// Resolves a pattern and brings its binders into scope. A binder is
    /// numbered before any that its inner pattern binds.
    fn pattern(&mut self, pattern: &Pattern) -> Result<Pattern<Reference>, Error> {
        Ok(match pattern {
            Pattern::Wildcard => Pattern::Wildcard,
            Pattern::Literal(literal) => Pattern::Literal(literal.clone()),
            Pattern::Record(fields) => Pattern::Record(
                fields
                    .iter()
                    .map(|(name, pattern)| Ok((name.clone(), self.pattern(pattern)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Pattern::Tuple(members) => Pattern::Tuple(
                members
                    .iter()
                    .map(|member| self.pattern(member))
                    .collect::<Result<_, Error>>()?,
            ),
            Pattern::Is(ty) => Pattern::Is(self.ty(ty)?),
            Pattern::Bind(name, inner) => {
                self.bind(name);
                Pattern::Bind(name.clone(), Box::new(self.pattern(inner)?))
            }
        })
    }

    pub(super) fn exprs(&mut self, exprs: &[Expr]) -> Result<Vec<Expr<Reference>>, Error> {
        exprs.iter().map(|expr| self.expr(expr)).collect()
    }

    /// Resolves an expression, and notes it as where an error is
    /// unless an expression inside it already was.
    pub(super) fn expr(&mut self, expr: &Expr) -> Result<Expr<Reference>, Error> {
        let resolved = self.node(expr);
        if resolved.is_err() && self.blamed.is_none() {
            self.blamed = Some(std::ptr::from_ref(expr));
        }
        resolved
    }

    fn node(&mut self, expr: &Expr) -> Result<Expr<Reference>, Error> {
        Ok(match expr {
            Expr::Null => Expr::Null,
            Expr::Literal(literal) => Expr::Literal(literal.clone()),
            Expr::Name(path) => Expr::Name(self.value(path, self.name(path)?)?),
            Expr::Instantiate { function, args } => Expr::Instantiate {
                function: self.value(function, self.name(function)?)?,
                args: self.types(args)?,
            },
            Expr::Call { callee, args } => match self.intrinsic(callee, args) {
                Some(node) => node?,
                None => Expr::Call {
                    callee: Box::new(self.expr(callee)?),
                    args: self.exprs(args)?,
                },
            },
            Expr::Record(fields) => Expr::Record(
                fields
                    .iter()
                    .map(|(key, value)| Ok((self.key(key)?, self.expr(value)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Expr::Method(function) => Expr::Method(Box::new(self.expr(function)?)),
            Expr::Tuple(members) => Expr::Tuple(self.exprs(members)?),
            Expr::List(members) => Expr::List(self.exprs(members)?),
            Expr::Field { target, key } => Expr::Field {
                target: Box::new(self.expr(target)?),
                key: self.key(key)?,
            },
            Expr::Send { .. } => self.send(expr)?,
            Expr::Merge { left, right } => Expr::Merge {
                left: Box::new(self.expr(left)?),
                right: Box::new(self.expr(right)?),
            },
            Expr::Let { name, value, body } => self.let_(name, value, body)?,
            Expr::Using { name, value, body } => self.using(name, value, body)?,
            // The reader takes its marks out, so one here is a tree that
            // was never built.
            Expr::At(..) => return Err(Error::UnknownSymbol(self.symbol.clone())),
            Expr::Operation { operator, operands } => Expr::Operation {
                operator: *operator,
                operands: operands
                    .iter()
                    .map(|operand| self.expr(operand))
                    .collect::<Result<_, _>>()?,
            },
            Expr::Match { subject, arms } => Expr::Match {
                subject: Box::new(self.expr(subject)?),
                arms: self.arms(arms)?,
            },
            Expr::Perform { ability, args } => Expr::Perform {
                ability: self.ability(ability)?,
                args: self.exprs(args)?,
            },
            Expr::Observe { observer, body } => {
                let observer = Box::new(self.expr(observer)?);
                // The binder a recording holds its observer in.
                self.next += 1;
                Expr::Observe {
                    observer,
                    body: Box::new(self.expr(body)?),
                }
            }
            Expr::AbilityId(ability) => Expr::AbilityId(self.ability(ability)?),
            Expr::Group(body) => Expr::Group(Box::new(self.expr(body)?)),
            Expr::Cell { .. } => self.block(expr)?,
            Expr::Boundary(body) => Expr::Boundary(Box::new(self.expr(body)?)),
            // A key is a tag, which is named as a type is.
            Expr::Annotated { key, value, expr } => Expr::Annotated {
                key: Reference::Definition(self.lookup(Namespace::Type, key)?),
                value: Box::new(self.expr(value)?),
                expr: Box::new(self.expr(expr)?),
            },
            Expr::Spawn(task) => Expr::Spawn(Box::new(self.expr(task)?)),
            Expr::All(branches) => Expr::All(self.exprs(branches)?),
            Expr::First(branches) => Expr::First(self.exprs(branches)?),
            Expr::Provide { bundle, only, body } => {
                let bundle = Box::new(self.expr(bundle)?);
                // The binder a provide holds its bundle in. It has no name.
                self.next += 1;
                Expr::Provide {
                    bundle,
                    only: *only,
                    body: Box::new(self.expr(body)?),
                }
            }
            Expr::Lambda { type_params, .. } => {
                // Its type parameters follow those in scope, and a name
                // of its own hides one outside it.
                let outer = self.type_params.len();
                let names = type_params.iter().map(|param| param.name.clone());
                self.type_params.extend(names);
                let lambda = self.lambda(expr);
                self.type_params.truncate(outer);
                lambda?
            }
            Expr::Comptime(call) => Expr::Comptime(Box::new(self.comptime(call)?)),
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => Expr::Cast {
                value: Box::new(self.expr(value)?),
                ty: self.ty(ty)?,
                otherwise: Box::new(self.expr(otherwise)?),
            },
        })
    }

    fn send(&mut self, send: &Expr) -> Result<Expr<Reference>, Error> {
        let Expr::Send {
            target,
            method,
            args,
            ..
        } = send
        else {
            return self.expr(send);
        };
        Ok(Expr::Send {
            target: Box::new(self.expr(target)?),
            method: self.key(method)?,
            args: self.exprs(args)?,
        })
    }

    pub(super) fn key(&self, key: &Key) -> Result<Key<Reference>, Error> {
        Ok(match key {
            Key::Field(name) => Key::Field(name.clone()),
            // A key is named where an ability is: an ability if there
            // is one of that name, and a key if not.
            Key::Ability(path) => Key::Ability(self.ability(path).or_else(|unknown| {
                let key = self.lookup(Namespace::Contract, path);
                key.map(Reference::Definition).map_err(|_| unknown)
            })?),
        })
    }

    /// A cell operation resolves what it holds and binds nothing.
    fn block(&mut self, expr: &Expr) -> Result<Expr<Reference>, Error> {
        Ok(match expr {
            Expr::Cell {
                operation,
                ty,
                args,
            } => Expr::Cell {
                operation: *operation,
                ty: ty.as_ref().map(|ty| self.ty(ty)).transpose()?,
                args: self.exprs(args)?,
            },
            other => self.expr(other)?,
        })
    }

    /// A using binds as a let does, and then a binder no program
    /// names: what releases the value.
    fn using(&mut self, name: &str, value: &Expr, body: &Expr) -> Result<Expr<Reference>, Error> {
        let value = Box::new(self.expr(value)?);
        let scope = self.locals.len();
        self.bind(name);
        self.next += 1;
        let body = Box::new(self.expr(body)?);
        self.locals.truncate(scope);
        Ok(Expr::Using {
            name: name.to_owned(),
            value,
            body,
        })
    }

    /// A binding, in scope for its body and not for its value.
    fn let_(&mut self, name: &str, value: &Expr, body: &Expr) -> Result<Expr<Reference>, Error> {
        let value = Box::new(self.expr(value)?);
        let scope = self.locals.len();
        self.bind(name);
        let body = Box::new(self.expr(body)?);
        self.locals.truncate(scope);
        Ok(Expr::Let {
            name: name.to_owned(),
            value,
            body,
        })
    }

    /// Each arm, with what its pattern binds in scope for its body.
    fn arms(&mut self, arms: &[Arm]) -> Result<Vec<Arm<Reference>>, Error> {
        let mut resolved = Vec::new();
        for arm in arms {
            let scope = self.locals.len();
            let pattern = self.pattern(&arm.pattern)?;
            let body = self.expr(&arm.body)?;
            self.locals.truncate(scope);
            resolved.push(Arm { pattern, body });
        }
        Ok(resolved)
    }

    fn lambda(&mut self, lambda: &Expr) -> Result<Expr<Reference>, Error> {
        let Expr::Lambda {
            type_params,
            params,
            row,
            returns,
            body,
        } = lambda
        else {
            return self.expr(lambda);
        };
        let mut bounds = Vec::new();
        for param in type_params {
            bounds.push(TypeParam {
                name: param.name.clone(),
                bound: param.bound.as_ref().map(|ty| self.ty(ty)).transpose()?,
            });
        }
        let returns = returns.as_ref().map(|ty| self.ty(ty)).transpose()?;
        let mut resolved = Vec::new();
        for (name, ty) in params {
            let ty = ty.as_ref().map(|ty| self.ty(ty)).transpose()?;
            resolved.push((name.clone(), ty));
        }
        let scope = self.locals.len();
        for (name, _) in params {
            self.bind(name);
        }
        let body = Box::new(self.expr(body)?);
        self.locals.truncate(scope);
        Ok(Expr::Lambda {
            type_params: bounds,
            params: resolved,
            row: self.row(row)?,
            returns,
            body,
        })
    }
}
