//! Resolve: a surface tree and the definition tables it imports from yield
//! symbols. See `spec/symbols.md`.

use serde::{Deserialize, Serialize};

use super::read::Scope;
use super::{Engine, Query, Stage, memo};
use crate::error::Error;
use crate::hash::{Hash, Hasher, digest};
use crate::surface::{
    Ability, Arm, Definition, DefinitionKind, Expr, Function, Key, Param, Path, Pattern, Reference,
    Type, TypeDefinition, TypeParam,
};
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
                    type_params: Vec::new(),
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
    /// The type parameters of the definition, by position.
    type_params: Vec<String>,
    /// Binders in scope, innermost last, with their numbers.
    locals: Vec<(String, usize)>,
    next: usize,
}

impl Resolver<'_> {
    fn definition(&mut self, definition: &Definition) -> Result<Definition<Reference>, Error> {
        let kind = match &definition.kind {
            DefinitionKind::Type(definition) => {
                self.type_params.clone_from(&definition.params);
                DefinitionKind::Type(TypeDefinition {
                    params: definition.params.clone(),
                    ty: self.ty(&definition.ty)?,
                })
            }
            DefinitionKind::Function(function) => {
                DefinitionKind::Function(self.function(function)?)
            }
            DefinitionKind::Ability(ability) => DefinitionKind::Ability(Ability {
                tag: ability.tag.clone(),
                params: self.params(&ability.params)?,
                ret: self.ty(&ability.ret)?,
            }),
        };
        Ok(Definition {
            name: definition.name.clone(),
            public: definition.public,
            doc: definition.doc.clone(),
            kind,
        })
    }

    fn function(&mut self, function: &Function) -> Result<Function<Reference>, Error> {
        // A bound may mention any type parameter of the function.
        self.type_params = function
            .type_params
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let mut type_params = Vec::new();
        for param in &function.type_params {
            type_params.push(TypeParam {
                name: param.name.clone(),
                bound: param
                    .bound
                    .as_ref()
                    .map(|bound| self.ty(bound))
                    .transpose()?,
            });
        }
        let params = self.params(&function.params)?;
        let ret = self.ty(&function.ret)?;
        let row = self.row(&function.row)?;
        for param in &function.params {
            self.bind(&param.name);
        }
        let body = self.expr(&function.body)?;
        Ok(Function {
            type_params,
            params,
            ret,
            row,
            body,
        })
    }

    fn params(&self, params: &[Param]) -> Result<Vec<Param<Reference>>, Error> {
        params
            .iter()
            .map(|param| {
                Ok(Param {
                    name: param.name.clone(),
                    ty: self.ty(&param.ty)?,
                })
            })
            .collect()
    }

    fn ability(&self, path: &Path) -> Result<Reference, Error> {
        self.lookup(Namespace::Ability, path)
            .map(Reference::Definition)
    }

    fn row(&self, row: &[Path]) -> Result<Vec<Reference>, Error> {
        row.iter().map(|path| self.ability(path)).collect()
    }

    fn bind(&mut self, name: &str) {
        self.locals.push((name.to_owned(), self.next));
        self.next += 1;
    }

    fn types(&self, types: &[Type]) -> Result<Vec<Type<Reference>>, Error> {
        types.iter().map(|ty| self.ty(ty)).collect()
    }

    fn ty(&self, ty: &Type) -> Result<Type<Reference>, Error> {
        Ok(match ty {
            Type::Never => Type::Never,
            Type::Unknown => Type::Unknown,
            Type::Null => Type::Null,
            Type::Bool => Type::Bool,
            Type::Int => Type::Int,
            Type::Float => Type::Float,
            Type::String => Type::String,
            Type::Literal(literal) => Type::Literal(literal.clone()),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(key, ty)| Ok((self.key(key)?, self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Type::Tuple(members) => Type::Tuple(self.types(members)?),
            Type::List(member) => Type::List(Box::new(self.ty(member)?)),
            Type::Function { params, ret, row } => Type::Function {
                params: self.types(params)?,
                ret: Box::new(self.ty(ret)?),
                row: self.row(row)?,
            },
            Type::Union(members) => Type::Union(self.types(members)?),
            Type::Named(path, args) => {
                let parameter = match path.0.as_slice() {
                    [name] => self.type_params.iter().position(|param| param == name),
                    _ => None,
                };
                let reference = match parameter {
                    Some(position) => Reference::Parameter(position),
                    None => Reference::Definition(self.lookup(Namespace::Type, path)?),
                };
                Type::Named(reference, self.types(args)?)
            }
        })
    }

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

    fn exprs(&mut self, exprs: &[Expr]) -> Result<Vec<Expr<Reference>>, Error> {
        exprs.iter().map(|expr| self.expr(expr)).collect()
    }

    fn expr(&mut self, expr: &Expr) -> Result<Expr<Reference>, Error> {
        Ok(match expr {
            Expr::Null => Expr::Null,
            Expr::Literal(literal) => Expr::Literal(literal.clone()),
            Expr::Name(path) => Expr::Name(self.name(path)?),
            Expr::Instantiate { function, args } => Expr::Instantiate {
                function: Reference::Definition(self.lookup(Namespace::Value, function)?),
                args: self.types(args)?,
            },
            Expr::Call { callee, args } => Expr::Call {
                callee: Box::new(self.expr(callee)?),
                args: self.exprs(args)?,
            },
            Expr::Record(fields) => Expr::Record(
                fields
                    .iter()
                    .map(|(key, value)| Ok((self.key(key)?, self.expr(value)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Expr::Tuple(members) => Expr::Tuple(self.exprs(members)?),
            Expr::List(members) => Expr::List(self.exprs(members)?),
            Expr::Field { target, key } => Expr::Field {
                target: Box::new(self.expr(target)?),
                key: self.key(key)?,
            },
            Expr::Merge { left, right } => Expr::Merge {
                left: Box::new(self.expr(left)?),
                right: Box::new(self.expr(right)?),
            },
            Expr::Let { name, value, body } => {
                let value = Box::new(self.expr(value)?);
                let scope = self.locals.len();
                self.bind(name);
                let body = Box::new(self.expr(body)?);
                self.locals.truncate(scope);
                Expr::Let {
                    name: name.clone(),
                    value,
                    body,
                }
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => Expr::Binary {
                operator: *operator,
                left: Box::new(self.expr(left)?),
                right: Box::new(self.expr(right)?),
            },
            Expr::Match { subject, arms } => {
                let subject = Box::new(self.expr(subject)?);
                let mut resolved = Vec::new();
                for arm in arms {
                    let scope = self.locals.len();
                    let pattern = self.pattern(&arm.pattern)?;
                    let body = self.expr(&arm.body)?;
                    self.locals.truncate(scope);
                    resolved.push(Arm { pattern, body });
                }
                Expr::Match {
                    subject,
                    arms: resolved,
                }
            }
            Expr::Perform { ability, args } => Expr::Perform {
                ability: self.ability(ability)?,
                args: self.exprs(args)?,
            },
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
            Expr::Lambda { params, row, body } => self.lambda(params, row, body)?,
            Expr::Comptime(call) => Expr::Comptime(Box::new(self.expr(call)?)),
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

    fn key(&self, key: &Key) -> Result<Key<Reference>, Error> {
        Ok(match key {
            Key::Field(name) => Key::Field(name.clone()),
            Key::Ability(path) => Key::Ability(self.ability(path)?),
        })
    }

    fn lambda(
        &mut self,
        params: &[(String, Option<Type>)],
        row: &[Path],
        body: &Expr,
    ) -> Result<Expr<Reference>, Error> {
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
            params: resolved,
            row: self.row(row)?,
            body,
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
