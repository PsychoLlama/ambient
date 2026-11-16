//! Code as a tree: the expression a comptime call gave, checked where
//! it is spliced as one written there would be. See "Splice
//! positions" and "Trees as values" in `spec/comptime.md`.

use super::reify::{definition, each, field, text, written};
use super::{Checker, core};
use crate::error::Error;
use crate::literal::Literal;
use crate::surface::{
    Ability, Definition, DefinitionKind, Expr, Function, Key, Param, Reference, TypeDefinition,
};
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
        let mut reader = Reader {
            next: base,
            bound: Vec::new(),
        };
        let Some(code) = reader.expression(tree) else {
            return Err(Error::NotCode(self.symbol.clone()));
        };
        let mut named = Vec::new();
        code.names(&mut |name| {
            if let Reference::Definition(symbol) = name {
                named.push(symbol.clone());
            }
        });
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

/// The function a `tree.Function` a call gave stands for, if it
/// stands for one. Its parameters are numbered as resolution numbers
/// those of a function that was written: first, and from nothing.
#[must_use]
pub fn defined(tree: &core::Expr) -> Option<Function<Reference>> {
    let core::Expr::Record(fields) = tree else {
        return None;
    };
    let mut reader = Reader {
        next: 0,
        bound: Vec::new(),
    };
    let mut params = Vec::new();
    for ty in each(field(fields, "params")?, written)? {
        params.push(Param {
            name: format!("__{}", params.len()),
            ty,
        });
        reader.bind();
    }
    Some(Function {
        type_params: Vec::new(),
        params,
        ret: written(field(fields, "returns")?)?,
        row: each(field(fields, "takes")?, |taken| {
            definition(taken, &["ability"])
        })?,
        body: reader.expression(field(fields, "body")?)?,
    })
}

/// The definitions a `tree.Module` a call gave stands for, if it
/// stands for any: each as resolution would leave one that was
/// written. A name a program could not write is none.
#[must_use]
pub fn definitions(tree: &core::Expr) -> Option<Vec<Definition<Reference>>> {
    each(tree, |entry| {
        let core::Expr::Record(fields) = entry else {
            return None;
        };
        let name = text(field(fields, "name")?)?;
        let core::Expr::Literal(Literal::Bool(public)) = field(fields, "public")? else {
            return None;
        };
        let kind = kind(fields)?;
        let writable = !name.is_empty() && !name.starts_with("__");
        writable.then(|| Definition {
            name: name.to_owned(),
            public: *public,
            doc: None,
            kind,
        })
    })
}

/// What a `tree.Definition` with these fields is. A tag is left a
/// tag: what it means holds where it is declared, which the engine
/// knows.
fn kind(fields: &[(core::Key, core::Expr)]) -> Option<DefinitionKind<Reference>> {
    let has = |name: &str| field(fields, name);
    Some(if let Some(function) = has("function") {
        DefinitionKind::Function(defined(function)?)
    } else if let Some(takes) = has("ability") {
        let param = |(at, ty)| Param {
            name: format!("__{at}"),
            ty,
        };
        DefinitionKind::Ability(Ability {
            type_params: Vec::new(),
            params: each(takes, written)?
                .into_iter()
                .enumerate()
                .map(param)
                .collect(),
            ret: written(has("returns")?)?,
        })
    } else if has("tag").is_some() {
        DefinitionKind::Tag
    } else {
        DefinitionKind::Type(TypeDefinition {
            params: Vec::new(),
            ty: written(has("means")?)?,
        })
    })
}

/// What reads a spliced `tree.Expr`. A tree names a local by its
/// place among those bound around it; an expression names one by a
/// number given in the order binders are met, which is the order the
/// checker binds them in.
struct Reader {
    /// The number the next binder is given.
    next: usize,
    /// The number of each local bound here, outermost first.
    bound: Vec<usize>,
}

impl Reader {
    fn bind(&mut self) {
        self.bound.push(self.next);
        self.next += 1;
    }

    fn all(&mut self, trees: &core::Expr) -> Option<Vec<Expr<Reference>>> {
        let core::Expr::List(trees) = trees else {
            return None;
        };
        trees.iter().map(|tree| self.expression(tree)).collect()
    }

    fn boxed(&mut self, tree: Option<&core::Expr>) -> Option<Box<Expr<Reference>>> {
        self.expression(tree?).map(Box::new)
    }

    /// The expression a spliced `tree.Expr` stands for, if it stands
    /// for one.
    fn expression(&mut self, tree: &core::Expr) -> Option<Expr<Reference>> {
        let core::Expr::Record(fields) = tree else {
            return None;
        };
        let has = |name: &str| field(fields, name);
        Some(match fields.as_slice() {
            [(core::Key::Field(name), holds)] => self.one(name, holds)?,
            _ if has("call").is_some() => Expr::Call {
                callee: self.boxed(has("call"))?,
                args: self.all(has("given")?)?,
            },
            _ if has("perform").is_some() => Expr::Perform {
                ability: definition(has("perform")?, &["ability"])?,
                args: self.all(has("given")?)?,
            },
            _ if has("of").is_some() => Expr::Field {
                target: self.boxed(has("of"))?,
                key: Key::Field(text(has("field")?)?.to_owned()),
            },
            _ if has("cast").is_some() => Expr::Cast {
                value: self.boxed(has("cast"))?,
                ty: written(has("to")?)?,
                otherwise: self.boxed(has("otherwise"))?,
            },
            _ if has("bind").is_some() => {
                let value = self.boxed(has("bind"))?;
                let scope = self.bound.len();
                self.bind();
                let name = format!("__{scope}");
                let body = self.boxed(has("body"));
                self.bound.truncate(scope);
                Expr::Let {
                    name,
                    value,
                    body: body?,
                }
            }
            _ => self.function(has("function")?, has("body")?)?,
        })
    }

    /// A node that is one field: what it is, and what it holds.
    fn one(&mut self, name: &str, holds: &core::Expr) -> Option<Expr<Reference>> {
        Some(match (name, holds) {
            ("literal", core::Expr::Null) => Expr::Null,
            ("literal", core::Expr::Literal(literal)) => Expr::Literal(literal.clone()),
            ("definition", named) => Expr::Name(definition(named, &["value", "type"])?),
            ("identity", named) => Expr::AbilityId(definition(named, &["ability"])?),
            ("local", core::Expr::Literal(Literal::Int(place))) => {
                let place = usize::try_from(*place).ok()?;
                Expr::Name(Reference::Local(*self.bound.get(place)?))
            }
            ("items", items) => Expr::List(self.all(items)?),
            ("fields", core::Expr::List(entries)) => {
                let entries = entries.iter().map(|entry| self.entry(entry));
                Expr::Record(entries.collect::<Option<_>>()?)
            }
            _ => return None,
        })
    }

    /// A field of a record, or the handler it holds for an ability.
    fn entry(&mut self, entry: &core::Expr) -> Option<(Key<Reference>, Expr<Reference>)> {
        let core::Expr::Record(entry) = entry else {
            return None;
        };
        let key = match field(entry, "ability") {
            Some(ability) => Key::Ability(definition(ability, &["ability"])?),
            None => Key::Field(text(field(entry, "name")?)?.to_owned()),
        };
        Some((key, self.expression(field(entry, "value")?)?))
    }

    fn function(&mut self, params: &core::Expr, body: &core::Expr) -> Option<Expr<Reference>> {
        let params = each(params, written)?;
        let scope = self.bound.len();
        let mut typed = Vec::new();
        for ty in params {
            typed.push((format!("__{}", self.bound.len()), Some(ty)));
            self.bind();
        }
        let body = self.expression(body);
        self.bound.truncate(scope);
        Some(Expr::Lambda {
            type_params: Vec::new(),
            params: typed,
            row: Vec::new(),
            returns: None,
            body: Box::new(body?),
        })
    }
}
