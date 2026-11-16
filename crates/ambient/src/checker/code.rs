//! Code as a tree: the expression a comptime call gave, checked where
//! it is spliced as one written there would be. See "Splice
//! positions" and "Trees as values" in `spec/comptime.md`.

use super::reify::{definition, each, field, text, written};
use super::{Checker, core};
use crate::error::Error;
use crate::surface::{Expr, Key, Reference};
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
        let Some(code) = expression(tree, base, 0) else {
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

/// The expression a spliced `tree.Expr` stands for, if it stands for
/// one. Its locals are numbered from `base`, and `bound` of them are
/// bound where it is.
fn expression(tree: &core::Expr, base: usize, bound: usize) -> Option<Expr<Reference>> {
    let core::Expr::Record(fields) = tree else {
        return None;
    };
    let inside = |tree: &core::Expr| expression(tree, base, bound);
    let boxed = |name: &str| field(fields, name).and_then(inside).map(Box::new);
    Some(match fields.as_slice() {
        [(core::Key::Field(name), holds)] => match (name.as_str(), holds) {
            ("literal", core::Expr::Null) => Expr::Null,
            ("literal", core::Expr::Literal(literal)) => Expr::Literal(literal.clone()),
            ("definition", named) => Expr::Name(definition(named, &["value", "type"])?),
            ("local", core::Expr::Literal(crate::literal::Literal::Int(index))) => {
                let index = usize::try_from(*index)
                    .ok()
                    .filter(|index| *index < bound)?;
                Expr::Name(Reference::Local(base + index))
            }
            ("items", items) => Expr::List(each(items, inside)?),
            ("fields", entries) => Expr::Record(each(entries, |entry| {
                let core::Expr::Record(entry) = entry else {
                    return None;
                };
                let name = text(field(entry, "name")?)?.to_owned();
                Some((Key::Field(name), inside(field(entry, "value")?)?))
            })?),
            _ => return None,
        },
        _ if field(fields, "call").is_some() => Expr::Call {
            callee: boxed("call")?,
            args: each(field(fields, "given")?, inside)?,
        },
        _ if field(fields, "of").is_some() => Expr::Field {
            target: boxed("of")?,
            key: Key::Field(text(field(fields, "field")?)?.to_owned()),
        },
        _ => {
            let params = each(field(fields, "function")?, written)?;
            let inner = bound + params.len();
            let typed = |(index, ty): (usize, Ty)| (format!("__{}", bound + index), Some(ty));
            Expr::Lambda {
                type_params: Vec::new(),
                params: params.into_iter().enumerate().map(typed).collect(),
                row: Vec::new(),
                returns: None,
                body: Box::new(expression(field(fields, "body")?, base, inner)?),
            }
        }
    })
}
