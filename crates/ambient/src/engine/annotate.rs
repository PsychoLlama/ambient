//! Annotations: what a definition says to a build, and what a build
//! is told of them and of the definitions they are on. See
//! `spec/annotations.md` and "Build abilities" in `spec/tools.md`.

use std::collections::{HashMap, HashSet};

use super::Engine;
use super::asked::record;
use crate::checker;
use crate::core::{Annotation, Expr, Open, Slot, Tree};
use crate::error::Error;
use crate::exchange;
use crate::hash::Hasher;
use crate::literal::Literal;
use crate::surface::{DefinitionKind, Reference, Type, data};
use crate::symbol::Symbol;
use crate::types::Ty;
use crate::value::Value;

/// What a walk of a definition meets: something said to a build, or
/// another definition, which the walk goes on into.
enum Met {
    Said(Annotation),
    Named(Symbol),
}

/// What a walk of `open` meets, in order: what is said of the
/// definition, then its body as it is written, then whatever else it
/// refers to, as its types do.
fn met(open: &Open) -> Vec<Met> {
    let mut found: Vec<Met> = open.annotations.iter().cloned().map(Met::Said).collect();
    let named = |slot: &Slot| open.slots.get(slot.0).cloned().map(Met::Named);
    if let Tree::Function(function) = &open.tree {
        // What is left to walk, the next expression last.
        let mut pending = vec![&function.body];
        while let Some(expr) = pending.pop() {
            match expr {
                Expr::Annotated { annotation, .. } => found.push(Met::Said(annotation.clone())),
                Expr::Reference(slot, _)
                | Expr::Closure { slot, .. }
                | Expr::AbilityId(slot)
                | Expr::Perform { ability: slot, .. } => found.extend(named(slot)),
                _ => {}
            }
            let from = pending.len();
            expr.children(&mut |child| pending.push(child));
            pending[from..].reverse();
        }
    }
    found.extend(open.slots.iter().cloned().map(Met::Named));
    found
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
        self.walk(from, &mut |symbol, said| {
            let Some(said) = said.filter(|said| said.package == package && said.tag == tag) else {
                return;
            };
            // What was encoded here decodes.
            let value = exchange::decode(&said.value).unwrap_or(Value::Null);
            found.push(record([
                ("on", checker::reference(symbol)),
                ("value", value),
            ]));
        })?;
        Ok(Value::List(found))
    }

    /// Walks what `from` reaches, in the order `spec/annotations.md`
    /// gives: `visit` is told each definition as the walk comes to it,
    /// and then each thing said in it, with the definition it is in.
    pub(super) fn walk(
        &self,
        from: &Symbol,
        visit: &mut dyn FnMut(&Symbol, Option<&Annotation>),
    ) -> Result<(), Error> {
        let mut seen = HashSet::from([from.clone()]);
        visit(from, None);
        // The definitions being walked, each with what is left of it.
        let mut walking = vec![(from.clone(), met(&*self.check(from)?).into_iter())];
        while let Some((symbol, rest)) = walking.last_mut() {
            match rest.next() {
                Some(Met::Said(said)) => visit(symbol, Some(&said)),
                Some(Met::Named(named)) => {
                    if seen.insert(named.clone()) {
                        visit(&named, None);
                        let rest = met(&*self.check(&named)?).into_iter();
                        walking.push((named, rest));
                    }
                }
                None => {
                    walking.pop();
                }
            }
        }
        Ok(())
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
