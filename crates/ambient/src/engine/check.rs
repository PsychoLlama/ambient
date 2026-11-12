//! Check: a resolved tree and the interfaces it refers to yield an open core
//! definition. See `spec/definitions.md` and `spec/core.md`.
//!
//! TODO: this checks only that calls pass the right number of arguments.
//! The type checker replaces `validate`; the key it runs under is already
//! the right one.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Engine, Query, Stage, memo};
use crate::core::{self, Open, Slot, Tree, encode_signature};
use crate::error::Error;
use crate::hash::{Hash, Hasher};
use crate::surface::{Definition, DefinitionKind, Expr, Function, Reference, Type};
use crate::symbol::Symbol;

/// What dependents check against.
#[derive(Debug)]
pub struct Interface {
    pub kind: InterfaceKind,
    /// Changes exactly when a dependent must be rechecked.
    pub digest: Hash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceKind {
    Type,
    Function { arity: usize },
}

impl Engine<'_> {
    /// A definition's interface, from its header alone.
    ///
    /// # Errors
    ///
    /// Fails if the header does not resolve, or a type it names does not
    /// check.
    pub fn interface(&self, symbol: &Symbol) -> Query<Interface> {
        memo(&self.interface, symbol, || {
            let resolved = self.resolve(symbol)?;
            let interface = match &resolved.definition.kind {
                // A type's interface is the type itself.
                DefinitionKind::Type(_) => Interface {
                    kind: InterfaceKind::Type,
                    digest: self.hash(symbol)?,
                },
                DefinitionKind::Function(function) => {
                    let mut lowering = Lowering::default();
                    let (params, ret) = lowering.signature(function);
                    let mut digest = Hasher::new("interface");
                    digest.bytes(&encode_signature(&params, &ret));
                    for slot in &lowering.slots {
                        digest.hash(&self.hash(slot)?);
                    }
                    Interface {
                        kind: InterfaceKind::Function {
                            arity: function.params.len(),
                        },
                        digest: digest.finish(),
                    }
                }
            };
            Ok(Arc::new(interface))
        })
    }

    /// A definition's open core tree.
    ///
    /// # Errors
    ///
    /// Fails if the definition does not resolve or does not check.
    pub fn check(&self, symbol: &Symbol) -> Query<Open> {
        memo(&self.check, symbol, || {
            let resolved = self.resolve(symbol)?;
            let open = lower(&resolved.definition);

            let mut key = Hasher::new("check");
            key.hash(&resolved.digest);
            let mut interfaces = BTreeMap::new();
            // A type refers only to types, and its interface is itself, so
            // checking one depends on nothing but its own tree.
            if matches!(open.tree, Tree::Function(_)) {
                for slot in &open.slots {
                    let interface = self.interface(slot)?;
                    key.hash(&interface.digest);
                    interfaces.insert(slot.clone(), interface.kind);
                }
            }

            self.stage(Stage::Check, symbol, key.finish(), || {
                validate(symbol, &resolved.definition, &interfaces)?;
                Ok(open)
            })
        })
    }
}

fn validate(
    symbol: &Symbol,
    definition: &Definition<Reference>,
    interfaces: &BTreeMap<Symbol, InterfaceKind>,
) -> Result<(), Error> {
    match &definition.kind {
        DefinitionKind::Type(_) => Ok(()),
        DefinitionKind::Function(function) => validate_expr(symbol, &function.body, interfaces),
    }
}

fn validate_expr(
    symbol: &Symbol,
    expr: &Expr<Reference>,
    interfaces: &BTreeMap<Symbol, InterfaceKind>,
) -> Result<(), Error> {
    let each = |expr| validate_expr(symbol, expr, interfaces);
    match expr {
        Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::String(_)
        | Expr::Name(_) => Ok(()),
        Expr::Call { callee, args } => {
            if let Expr::Name(Reference::Definition(target)) = &**callee
                && let Some(InterfaceKind::Function { arity }) = interfaces.get(target)
                && *arity != args.len()
            {
                return Err(Error::Arity {
                    symbol: symbol.clone(),
                    callee: Box::new(target.clone()),
                    expected: *arity,
                    found: args.len(),
                });
            }
            each(callee)?;
            args.iter().try_for_each(each)
        }
        Expr::Record(fields) => fields.iter().try_for_each(|(_, value)| each(value)),
        Expr::Field { target, .. } => each(target),
        Expr::Let { value, body, .. } => {
            each(value)?;
            each(body)
        }
    }
}

fn lower(definition: &Definition<Reference>) -> Open {
    let mut lowering = Lowering::default();
    let tree = match &definition.kind {
        DefinitionKind::Type(ty) => Tree::Type(lowering.ty(ty)),
        DefinitionKind::Function(function) => {
            let (params, ret) = lowering.signature(function);
            Tree::Function(core::Function {
                params,
                ret,
                body: lowering.expr(&function.body),
            })
        }
    };
    Open {
        tree,
        slots: lowering.slots,
    }
}

/// Turns resolved trees into core trees, handing out slots in order of
/// first use.
#[derive(Default)]
struct Lowering {
    slots: Vec<Symbol>,
}

impl Lowering {
    fn slot(&mut self, symbol: &Symbol) -> Slot {
        let index = self
            .slots
            .iter()
            .position(|slot| slot == symbol)
            .unwrap_or_else(|| {
                self.slots.push(symbol.clone());
                self.slots.len() - 1
            });
        Slot(index)
    }

    fn signature(&mut self, function: &Function<Reference>) -> (Vec<core::Type>, core::Type) {
        let params = function.params.iter().map(|p| self.ty(&p.ty)).collect();
        (params, self.ty(&function.ret))
    }

    fn ty(&mut self, ty: &Type<Reference>) -> core::Type {
        match ty {
            // A resolved type never names a local; resolution has no way to
            // produce one.
            Type::Null | Type::Named(Reference::Local(_)) => core::Type::Null,
            Type::Bool => core::Type::Bool,
            Type::Int => core::Type::Int,
            Type::Float => core::Type::Float,
            Type::String => core::Type::String,
            Type::Record(fields) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), self.ty(ty)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                core::Type::Record(fields)
            }
            Type::Function { params, ret } => core::Type::Function {
                params: params.iter().map(|ty| self.ty(ty)).collect(),
                ret: Box::new(self.ty(ret)),
            },
            Type::Named(Reference::Definition(symbol)) => core::Type::Reference(self.slot(symbol)),
        }
    }

    fn expr(&mut self, expr: &Expr<Reference>) -> core::Expr {
        match expr {
            Expr::Null => core::Expr::Null,
            Expr::Bool(value) => core::Expr::Bool(*value),
            Expr::Int(value) => core::Expr::Int(*value),
            Expr::Float(value) => core::Expr::Float(*value),
            Expr::String(value) => core::Expr::String(value.clone()),
            Expr::Name(Reference::Local(number)) => core::Expr::Local(*number),
            Expr::Name(Reference::Definition(symbol)) => core::Expr::Reference(self.slot(symbol)),
            Expr::Call { callee, args } => core::Expr::Call {
                callee: Box::new(self.expr(callee)),
                args: args.iter().map(|arg| self.expr(arg)).collect(),
            },
            Expr::Record(fields) => core::Expr::Record(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), self.expr(value)))
                    .collect(),
            ),
            Expr::Field { target, name } => core::Expr::Field {
                target: Box::new(self.expr(target)),
                name: name.clone(),
            },
            Expr::Let { value, body, .. } => core::Expr::Let {
                value: Box::new(self.expr(value)),
                body: Box::new(self.expr(body)),
            },
        }
    }
}
