//! Resolve: a surface tree and the definition tables it imports from yield
//! symbols. See `spec/symbols.md`.

mod expr;

use serde::{Deserialize, Serialize};

use super::scope::Scope;
use super::{Engine, Query, Stage, memo};
use crate::error::Error;
use crate::hash::{Hash, Hasher, digest};
use crate::surface::{
    Ability, Definition, DefinitionKind, Extern, ExternKind, Function, KeyDefinition, Param, Path,
    Reference, Target, Type, TypeDefinition, TypeParam,
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
            let package = self.sources.identity(&symbol.module.package);

            let mut key = Hasher::new("resolve");
            key.hash(&module.digests[entry.index])
                .hash(&scoped.digest)
                .bytes(package.as_bytes());
            // What a tag resolves to holds where it is declared.
            if matches!(definition.kind, DefinitionKind::Tag) {
                key.bytes(symbol.identity().as_bytes());
            }
            let key = key.finish();
            self.stage(Stage::Resolve, symbol, key, || {
                let definition = Resolver {
                    scope: &scoped.scope,
                    symbol,
                    package: &package,
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
    /// The ID of the package the definition is in.
    package: &'a str,
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
            DefinitionKind::Ability(ability) => {
                let type_params = self.type_params(&ability.type_params)?;
                DefinitionKind::Ability(Ability {
                    type_params,
                    params: self.params(&ability.params)?,
                    ret: self.ty(&ability.ret)?,
                })
            }
            // A tag is, to everything after this, the type of its one
            // value: a tag told apart by where this is declared.
            DefinitionKind::Tag => DefinitionKind::Type(TypeDefinition {
                params: Vec::new(),
                ty: Type::Literal(crate::literal::Literal::Tag {
                    package: self.package.to_owned(),
                    tag: self.symbol.identity(),
                }),
            }),
            DefinitionKind::Key(key) => DefinitionKind::Key(KeyDefinition {
                ty: self.ty(&key.ty)?,
            }),
            DefinitionKind::Extern(native) => DefinitionKind::Extern(Extern {
                kind: match &native.kind {
                    ExternKind::Function { params, ret } => ExternKind::Function {
                        params: self.params(params)?,
                        ret: self.ty(ret)?,
                    },
                    ExternKind::Handler { ability, row } => ExternKind::Handler {
                        ability: self.ability(ability)?,
                        row: self.row(row)?,
                    },
                },
            }),
            DefinitionKind::Target(target) => {
                let value = |path| {
                    self.lookup(Namespace::Value, path)
                        .map(Reference::Definition)
                };
                let mut bindings = Vec::new();
                for (ability, handler) in &target.bindings {
                    bindings.push((self.ability(ability)?, value(handler)?));
                }
                DefinitionKind::Target(Target {
                    extends: target.extends.iter().map(value).collect::<Result<_, _>>()?,
                    bindings,
                })
            }
        };
        Ok(Definition {
            name: definition.name.clone(),
            public: definition.public,
            doc: definition.doc.clone(),
            kind,
        })
    }

    /// The type parameters of a definition, which are in scope from
    /// here on: a bound may mention any of them.
    fn type_params(&mut self, written: &[TypeParam]) -> Result<Vec<TypeParam<Reference>>, Error> {
        self.type_params = written.iter().map(|p| p.name.clone()).collect();
        let mut type_params = Vec::new();
        for param in written {
            type_params.push(TypeParam {
                name: param.name.clone(),
                bound: param
                    .bound
                    .as_ref()
                    .map(|bound| self.ty(bound))
                    .transpose()?,
            });
        }
        Ok(type_params)
    }

    fn function(&mut self, function: &Function) -> Result<Function<Reference>, Error> {
        let type_params = self.type_params(&function.type_params)?;
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
            Type::Tag => Type::Tag,
            Type::Literal(literal) => Type::Literal(literal.clone()),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|(key, ty)| Ok((self.key(key)?, self.ty(ty)?)))
                    .collect::<Result<_, Error>>()?,
            ),
            Type::Tuple(members) => Type::Tuple(self.types(members)?),
            Type::List(member) => Type::List(Box::new(self.ty(member)?)),
            Type::Cell(member) => Type::Cell(Box::new(self.ty(member)?)),
            Type::Iterator(member) => Type::Iterator(Box::new(self.ty(member)?)),
            Type::Method { needs, function } => Type::Method {
                needs: Box::new(self.ty(needs)?),
                function: Box::new(self.ty(function)?),
            },
            Type::Function { params, ret, row } => Type::Function {
                params: self.types(params)?,
                ret: Box::new(self.ty(ret)?),
                row: self.row(row)?,
            },
            Type::Union(members) => Type::Union(self.types(members)?),
            Type::Bound(index) => Type::Bound(*index),
            Type::Generic { bounds, function } => Type::Generic {
                bounds: self.types(bounds)?,
                function: Box::new(self.ty(function)?),
            },
            Type::Named(path, args) => {
                let parameter = match path.0.as_slice() {
                    [name] => self.type_params.iter().rposition(|param| param == name),
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

    fn name(&self, path: &Path) -> Result<Reference, Error> {
        if let [name] = path.0.as_slice()
            && let Some((_, number)) = self.locals.iter().rev().find(|(local, _)| local == name)
        {
            return Ok(Reference::Local(*number));
        }
        // A tag is named as a type; where a value is expected, the name
        // of one is its value, which the checker sees to.
        self.lookup(Namespace::Value, path)
            .or_else(|unknown| self.lookup(Namespace::Type, path).map_err(|_| unknown))
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
                .find(namespace, name)
                .cloned()
                .ok_or_else(unknown),
            [module, name] => {
                let (_, members) = self.scope.modules.get(module).ok_or_else(unknown)?;
                let member = members
                    .0
                    .get(&(namespace, name.clone()))
                    .ok_or_else(unknown)?;
                if member.public {
                    Ok(member.symbol.clone())
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
