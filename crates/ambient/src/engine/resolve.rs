//! Resolve: a surface tree and the definition tables it imports from yield
//! symbols. See `spec/symbols.md`.

use serde::{Deserialize, Serialize};

use super::read::Scope;
use super::{Engine, Query, Stage, memo};
use crate::error::Error;
use crate::hash::{Hash, Hasher, digest};
use crate::surface::{Definition, DefinitionKind, Expr, Function, Param, Path, Reference, Type};
use crate::symbol::{Namespace, Symbol};

#[derive(Debug, Serialize, Deserialize)]
pub struct Resolved {
    pub definition: Definition<Reference>,
    /// A digest of `definition`, kept so no later stage re-derives it.
    pub digest: Hash,
}

impl Engine<'_> {
    /// A definition with every name replaced by what it refers to.
    ///
    /// # Errors
    ///
    /// Fails if the symbol does not exist or a name in it resolves to
    /// nothing visible.
    pub fn resolve(&self, symbol: &Symbol) -> Query<Resolved> {
        memo(&self.resolve, symbol, || {
            let module = self.read(&symbol.module)?;
            let entry = module
                .table
                .0
                .get(&(symbol.namespace, symbol.name.clone()))
                .ok_or_else(|| Error::UnknownSymbol(symbol.clone()))?;
            let definition = &module.definitions[entry.index];
            let scoped = self.scope(&symbol.module)?;

            let key = Hasher::new("resolve")
                .hash(&module.digests[entry.index])
                .hash(&scoped.digest)
                .finish();
            self.stage(Stage::Resolve, symbol, key, || {
                let definition = Resolver {
                    scope: &scoped.scope,
                    symbol,
                    locals: Vec::new(),
                    next: 0,
                }
                .definition(definition)?;
                let digest = digest("resolved", &definition);
                Ok(Resolved { definition, digest })
            })
        })
    }
}

struct Resolver<'a> {
    scope: &'a Scope,
    symbol: &'a Symbol,
    /// Binders in scope, innermost last, with their numbers.
    locals: Vec<(String, usize)>,
    next: usize,
}

impl Resolver<'_> {
    fn definition(&mut self, definition: &Definition) -> Result<Definition<Reference>, Error> {
        let kind = match &definition.kind {
            DefinitionKind::Type(ty) => DefinitionKind::Type(self.ty(ty)?),
            DefinitionKind::Function(function) => {
                DefinitionKind::Function(self.function(function)?)
            }
        };
        Ok(Definition {
            name: definition.name.clone(),
            public: definition.public,
            doc: definition.doc.clone(),
            kind,
        })
    }

    fn function(&mut self, function: &Function) -> Result<Function<Reference>, Error> {
        let mut params = Vec::new();
        for param in &function.params {
            params.push(Param {
                name: param.name.clone(),
                ty: self.ty(&param.ty)?,
            });
        }
        let ret = self.ty(&function.ret)?;
        for param in &function.params {
            self.bind(&param.name);
        }
        let body = self.expr(&function.body)?;
        Ok(Function { params, ret, body })
    }

    fn bind(&mut self, name: &str) {
        self.locals.push((name.to_owned(), self.next));
        self.next += 1;
    }

    fn ty(&self, ty: &Type) -> Result<Type<Reference>, Error> {
        Ok(match ty {
            Type::Null => Type::Null,
            Type::Bool => Type::Bool,
            Type::Int => Type::Int,
            Type::Float => Type::Float,
            Type::String => Type::String,
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(name, ty)| Ok((name.clone(), self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Type::Function { params, ret } => Type::Function {
                params: params
                    .iter()
                    .map(|ty| self.ty(ty))
                    .collect::<Result<_, Error>>()?,
                ret: Box::new(self.ty(ret)?),
            },
            Type::Named(path) => {
                Type::Named(Reference::Definition(self.lookup(Namespace::Type, path)?))
            }
        })
    }

    fn expr(&mut self, expr: &Expr) -> Result<Expr<Reference>, Error> {
        Ok(match expr {
            Expr::Null => Expr::Null,
            Expr::Bool(value) => Expr::Bool(*value),
            Expr::Int(value) => Expr::Int(*value),
            Expr::Float(value) => Expr::Float(*value),
            Expr::String(value) => Expr::String(value.clone()),
            Expr::Name(path) => Expr::Name(self.name(path)?),
            Expr::Call { callee, args } => Expr::Call {
                callee: Box::new(self.expr(callee)?),
                args: args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<_, Error>>()?,
            },
            Expr::Record(fields) => Expr::Record(
                fields
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), self.expr(value)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Expr::Field { target, name } => Expr::Field {
                target: Box::new(self.expr(target)?),
                name: name.clone(),
            },
            Expr::Let { name, value, body } => {
                let value = Box::new(self.expr(value)?);
                self.bind(name);
                let body = Box::new(self.expr(body)?);
                self.locals.pop();
                Expr::Let {
                    name: name.clone(),
                    value,
                    body,
                }
            }
        })
    }

    fn name(&self, path: &Path) -> Result<Reference, Error> {
        if let [name] = path.0.as_slice()
            && let Some((_, number)) = self.locals.iter().rev().find(|(local, _)| local == name)
        {
            return Ok(Reference::Local(*number));
        }
        self.lookup(Namespace::Value, path)
            .map(Reference::Definition)
    }

    /// A definition named directly, or through one imported module.
    fn lookup(&self, namespace: Namespace, path: &Path) -> Result<Symbol, Error> {
        let unknown = || Error::UnknownName {
            symbol: self.symbol.clone(),
            path: path.clone(),
        };
        match path.0.as_slice() {
            [name] => self
                .scope
                .definitions
                .get(&(namespace, name.clone()))
                .cloned()
                .ok_or_else(unknown),
            [module, name] => {
                let (id, table) = self.scope.modules.get(module).ok_or_else(unknown)?;
                let entry = table
                    .0
                    .get(&(namespace, name.clone()))
                    .ok_or_else(unknown)?;
                if entry.public {
                    Ok(id.symbol(namespace, name))
                } else {
                    Err(Error::PrivateName {
                        symbol: self.symbol.clone(),
                        path: path.clone(),
                    })
                }
            }
            _ => Err(unknown()),
        }
    }
}
