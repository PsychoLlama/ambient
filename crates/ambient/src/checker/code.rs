//! Code as a tree: the expression a `tree.Expr` a comptime call gave
//! stands for, checked where it is spliced as one written there would
//! be. See "Splice
//! positions" and "Trees as values" in `spec/comptime.md`.

use super::reify::{
    Home, Read, definition, each, field, fields, need, parameter_within, text, written,
};
use super::{Checker, core};
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{Arm, Expr, Key, Operator, Pattern, Reference, shown};
use crate::symbol::{Namespace, Symbol};
use crate::types::Ty;

/// Where the locals of spliced code are numbered from: past any a
/// definition binds, whose numbers resolution gave.
const SPLICED: usize = usize::MAX / 2;

impl Checker<'_> {
    /// The expression the spliced `tree.Expr` `tree` stands for, checked
    /// here.
    ///
    /// What it names must be what could be named here: a tree is data,
    /// and a function could make up a reference to what nothing may
    /// reach, such as an extern past the ability that guards it.
    pub(super) fn code(&mut self, tree: &core::Expr) -> Result<(core::Expr, Ty), Error> {
        let base = SPLICED + self.spliced;
        let code = Reader::new(None, base).expression(tree);
        let code = code.map_err(|why| Error::NotCode {
            symbol: self.symbol.clone(),
            why,
        })?;
        let mut named = Vec::new();
        // A type parameter it names is one of the definition it is
        // spliced in.
        let mut within = Ok(());
        let count = self.env.bounds.len();
        code.names(&mut |name| {
            within = within.clone().and(parameter_within(count, name));
            if let Reference::Definition(symbol) = name {
                named.push(symbol.clone());
            }
        });
        within.map_err(|why| Error::NotCode {
            symbol: self.symbol.clone(),
            why,
        })?;
        let from = &self.symbol.module.package;
        let hidden = |named: &&Symbol| !self.evaluator.is_some_and(|by| by.nameable(from, named));
        if let Some(hidden) = named.iter().find(hidden) {
            return Err(Error::Unnameable {
                symbol: self.symbol.clone(),
                named: hidden.clone(),
            });
        }
        // What the code names was not known when this check began: it
        // is asked for, and the check begun again with it.
        if !named.iter().all(|named| self.known(named)) {
            return Err(Error::Reaches {
                symbol: self.symbol.clone(),
                reached: named,
            });
        }
        self.reached.extend(named);
        let next = std::mem::replace(&mut self.next, base);
        let inferred = self.infer(&code);
        if inferred.is_err() && self.given.is_none() {
            // No node of the code is in the text: the error is at the
            // call, and the code is kept as text to show with it.
            let home = &self.symbol.module;
            let shown = shown::expression(home, base, &code, self.blamed.take());
            self.given = Some(shown.excerpt());
        }
        self.spliced += self.next - base;
        self.next = next;
        inferred
    }

    /// Whether checking has what it reads about a definition.
    fn known(&self, symbol: &Symbol) -> bool {
        match symbol.namespace {
            Namespace::Value => self.env.functions.contains_key(symbol),
            Namespace::Type => self.env.types.contains_key(symbol),
            Namespace::Ability => self.env.abilities.contains_key(symbol),
            Namespace::Key => self.env.keys.contains_key(symbol),
        }
    }
}

/// What reads a spliced `tree.Expr`. A tree names a local by its
/// place among those bound around it; an expression names one by a
/// number given in the order binders are met, which is the order the
/// checker binds them in.
pub(super) struct Reader<'a> {
    /// The module the code is a definition of, if a call gave the
    /// whole of one.
    home: Home<'a>,
    /// The number the next binder is given.
    next: usize,
    /// The number of each local bound here, outermost first.
    bound: Vec<usize>,
}

impl<'a> Reader<'a> {
    /// A reader of code inside nothing, whose binders are numbered
    /// from `next`.
    pub(super) fn new(home: Home<'a>, next: usize) -> Self {
        let bound = Vec::new();
        Self { home, next, bound }
    }

    /// Binds a local around whatever is read next.
    pub(super) fn bind(&mut self) {
        self.bound.push(self.next);
        self.next += 1;
    }

    fn all(&mut self, trees: &core::Expr) -> Read<Vec<Expr<Reference>>> {
        each(trees, |tree| self.expression(tree))
    }

    fn boxed(&mut self, tree: &core::Expr) -> Read<Box<Expr<Reference>>> {
        self.expression(tree).map(Box::new)
    }

    /// The expression a spliced `tree.Expr` stands for.
    pub(super) fn expression(&mut self, tree: &core::Expr) -> Read<Expr<Reference>> {
        let fields = fields(tree, "an expression")?;
        let has = |name: &str| field(fields, name).is_some();
        let need = |name: &str| need(fields, name);
        Ok(match fields {
            [(core::Key::Field(name), holds)] => self.one(name, holds)?,
            _ if has("call") => Expr::Call {
                callee: self.boxed(need("call")?)?,
                args: self.all(need("given")?)?,
            },
            _ if has("perform") => Expr::Perform {
                ability: definition(self.home, need("perform")?, &["ability"])?,
                args: self.all(need("given")?)?,
            },
            _ if has("method") => Expr::Send {
                target: self.boxed(need("of")?)?,
                method: Key::Field(text(need("method")?)?.to_owned()),
                args: self.all(need("given")?)?,
            },
            _ if has("of") => Expr::Field {
                target: self.boxed(need("of")?)?,
                key: Key::Field(text(need("field")?)?.to_owned()),
            },
            _ if has("cast") => Expr::Cast {
                value: self.boxed(need("cast")?)?,
                ty: written(self.home, need("to")?)?,
                otherwise: self.boxed(need("otherwise")?)?,
            },
            _ if has("subject") => Expr::Match {
                subject: self.boxed(need("subject")?)?,
                arms: each(need("arms")?, |arm| self.arm(arm))?,
            },
            _ if has("operation") => {
                let word = text(need("operation")?)?;
                let operator = Operator::named(word);
                Expr::Operation {
                    operator: operator.ok_or_else(|| format!("`{word}` is no operation"))?,
                    operands: self.all(need("on")?)?,
                }
            }
            _ if has("handlers") => Expr::Provide {
                bundle: self.boxed(need("handlers")?)?,
                only: false,
                body: self.boxed(need("body")?)?,
            },
            _ if has("bind") => {
                let value = self.boxed(need("bind")?)?;
                let scope = self.bound.len();
                self.bind();
                let name = format!("__{scope}");
                let body = self.boxed(need("body")?);
                self.bound.truncate(scope);
                Expr::Let {
                    name,
                    value,
                    body: body?,
                }
            }
            _ => self.function(need("function")?, need("body")?)?,
        })
    }

    /// A node that is one field: what it is, and what it holds.
    fn one(&mut self, name: &str, holds: &core::Expr) -> Read<Expr<Reference>> {
        Ok(match (name, holds) {
            ("literal", core::Expr::Null) => Expr::Null,
            ("literal", core::Expr::Literal(literal)) => Expr::Literal(literal.clone()),
            ("definition", named) => Expr::Name(definition(self.home, named, &["value", "type"])?),
            ("identity", named) => Expr::AbilityId(definition(self.home, named, &["ability"])?),
            ("local", core::Expr::Literal(Literal::Int(place))) => {
                let bound = usize::try_from(*place).ok();
                let bound = bound.and_then(|place| self.bound.get(place));
                Expr::Name(Reference::Local(*bound.ok_or_else(|| {
                    let bound = self.bound.len();
                    format!("local {place} is named where {bound} are bound")
                })?))
            }
            ("items", items) => Expr::List(self.all(items)?),
            ("members", members) => Expr::Tuple(self.all(members)?),
            ("fields", entries) => Expr::Record(each(entries, |entry| self.entry(entry))?),
            _ => return Err(format!("an expression is no `{name}`")),
        })
    }

    /// An arm of a match: what its pattern binds is bound in what it
    /// gives, and nowhere after.
    fn arm(&mut self, arm: &core::Expr) -> Read<Arm<Reference>> {
        let arm = fields(arm, "an arm")?;
        let scope = self.bound.len();
        let pattern = self.pattern(need(arm, "pattern")?);
        let body = pattern.and_then(|pattern| {
            let body = self.expression(need(arm, "gives")?)?;
            Ok(Arm { pattern, body })
        });
        self.bound.truncate(scope);
        body
    }

    /// The pattern a spliced `tree.Pattern` stands for. Each binding is
    /// numbered as it is met: one before what is inside it, which is
    /// the order resolution numbers those of a pattern that was
    /// written.
    fn pattern(&mut self, tree: &core::Expr) -> Read<Pattern<Reference>> {
        let [(core::Key::Field(name), holds)] = fields(tree, "a pattern")? else {
            return Err("a pattern is one field".to_owned());
        };
        Ok(match (name.as_str(), holds) {
            ("is", ty) => Pattern::Is(written(self.home, ty)?),
            ("equals", core::Expr::Literal(literal)) => Pattern::Literal(literal.clone()),
            ("bound", inner) => {
                let name = format!("__{}", self.bound.len());
                self.bind();
                Pattern::Bind(name, Box::new(self.pattern(inner)?))
            }
            ("tuple", members) => Pattern::Tuple(each(members, |member| self.pattern(member))?),
            ("shape", shaped) => Pattern::Record(each(shaped, |shaped| {
                let shaped = fields(shaped, "a field of a pattern")?;
                let name = text(need(shaped, "name")?)?.to_owned();
                Ok((name, self.pattern(need(shaped, "matches")?)?))
            })?),
            _ => return Err(format!("a pattern is no `{name}`")),
        })
    }

    /// A field of a record, or the handler it holds for an ability.
    fn entry(&mut self, entry: &core::Expr) -> Read<(Key<Reference>, Expr<Reference>)> {
        let entry = fields(entry, "an entry")?;
        let key = match field(entry, "ability") {
            Some(ability) => Key::Ability(definition(self.home, ability, &["ability"])?),
            None => Key::Field(text(need(entry, "name")?)?.to_owned()),
        };
        Ok((key, self.expression(need(entry, "value")?)?))
    }

    fn function(&mut self, params: &core::Expr, body: &core::Expr) -> Read<Expr<Reference>> {
        let params = each(params, |ty| written(self.home, ty))?;
        let scope = self.bound.len();
        let mut typed = Vec::new();
        for ty in params {
            typed.push((format!("__{}", self.bound.len()), Some(ty)));
            self.bind();
        }
        let body = self.expression(body);
        self.bound.truncate(scope);
        Ok(Expr::Lambda {
            type_params: Vec::new(),
            params: typed,
            row: Vec::new(),
            returns: None,
            body: Box::new(body?),
        })
    }
}
