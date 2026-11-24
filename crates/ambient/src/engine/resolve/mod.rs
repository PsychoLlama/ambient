//! Resolve: a surface tree and the definition tables it imports from yield
//! symbols. See `docs/spec/symbols.md`.

mod expr;
mod intrinsic;
mod lookup;
mod resolved;
mod types;

pub use self::resolved::Resolved;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::scope::{Members, Scope};
use super::{Engine, Query, Stage, memo};
use crate::error::Error;
use crate::hash::{Hasher, digest};
use crate::surface::{
    Ability, Annotation, ContractDefinition, Definition, DefinitionKind, Expr, Extern, Function,
    Param, Path, Reference, Target, Type, TypeDefinition, TypeParam,
};
use crate::symbol::{ModuleId, Namespace, Symbol};

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
            let Some(entry) = module.table.0.get(&(symbol.namespace, symbol.name.clone())) else {
                return self.resolve_made(symbol);
            };
            let definition = &module.definitions[entry.index];
            let scoped = self.scope(&symbol.module)?;
            let package = self.sources.identity(&symbol.module.package);
            let given = self.given_to(&scoped.scope, symbol, &package, definition)?;

            let mut key = Hasher::new("resolve");
            key.hash(&module.digests[entry.index])
                .hash(&scoped.digest)
                .bytes(package.as_bytes());
            // What a generated module has is no part of the scope.
            for (module, members) in &given {
                key.hash(&digest("module", module))
                    .hash(&digest("members", &**members));
            }
            // What a tag resolves to holds where it is declared.
            if matches!(definition.kind, DefinitionKind::Tag | DefinitionKind::Brand) {
                key.bytes(symbol.identity().as_bytes());
            }
            let key = key.finish();
            self.stage(Stage::Resolve, symbol, key, || {
                let mut resolver = Resolver::new(&scoped.scope, symbol, &package);
                resolver.given = given;
                let written = definition;
                let definition = resolver.definition(written)?;
                let skipped = body_of(written)
                    .map(|body| intrinsic::numbered(body, &resolver.dropped))
                    .unwrap_or_default();
                let added = body_of(&definition)
                    .map(|body| intrinsic::numbered(body, &resolver.added))
                    .unwrap_or_default();
                let stood = body_of(&definition)
                    .map(|body| intrinsic::stood(body, &resolver.stood))
                    .unwrap_or_default();
                let digest = digest("resolved", &definition);
                Ok(Resolved {
                    definition,
                    digest,
                    skipped,
                    added,
                    stood,
                })
            })
        })
    }

    /// The members of each generated module a definition names
    /// something through: what resolving it reads beside its scope.
    ///
    /// Only resolving says which those are, since a local may have the
    /// name of an import. So the definition is resolved with what is
    /// known so far, and again with whatever that asked for, until it
    /// asks for no more.
    fn given_to(
        &self,
        scope: &Scope,
        symbol: &Symbol,
        package: &str,
        definition: &Definition,
    ) -> Result<Given, Error> {
        let mut given = Given::new();
        while !scope.generated.is_empty() {
            let mut resolver = Resolver::new(scope, symbol, package);
            resolver.given = given.clone();
            // What is wrong with it is said when it is resolved for
            // good.
            let _ = resolver.definition(definition);
            let wanted = resolver.wanted.take();
            if wanted.is_empty() {
                break;
            }
            for module in wanted {
                let members = self.members(&module)?;
                given.insert(module, members);
            }
        }
        Ok(given)
    }
}

impl Engine<'_> {
    /// A definition no module was written with: one a generated
    /// module's call gave, which names what it refers to already.
    fn resolve_made(&self, symbol: &Symbol) -> Query<Resolved> {
        let made = self.made(&symbol.module)?;
        let named = |made: &&Definition<Reference>| {
            made.name == symbol.name && made.kind.namespace() == symbol.namespace
        };
        let definition = made.iter().find(named);
        let definition = definition.ok_or_else(|| Error::UnknownSymbol(symbol.clone()))?;
        Ok(std::sync::Arc::new(Resolved {
            definition: definition.clone(),
            digest: digest("resolved", definition),
            skipped: Vec::new(),
            added: Vec::new(),
            stood: Vec::new(),
        }))
    }

    /// The node of a function's body that keeps it from resolving, if
    /// one does: its place in the order [`Expr::children`] walks them.
    /// Resolving is done again to find it, since only an error is kept.
    pub(super) fn unresolved_at(&self, symbol: &Symbol) -> Option<usize> {
        let module = self.read(&symbol.module).ok()?;
        let entry = module
            .table
            .0
            .get(&(symbol.namespace, symbol.name.clone()))?;
        let definition = &module.definitions[entry.index];
        let (DefinitionKind::Function(function) | DefinitionKind::Constant(function)) =
            &definition.kind
        else {
            return None;
        };
        let scoped = self.scope(&symbol.module).ok()?;
        let package = self.sources.identity(&symbol.module.package);
        let given = self.given_to(&scoped.scope, symbol, &package, definition);
        let mut resolver = Resolver::new(&scoped.scope, symbol, &package);
        resolver.given = given.ok()?;
        resolver.definition(definition).err()?;
        function.body.index_of(resolver.blamed?)
    }
}

/// The body of a definition whose nodes are numbered: a function's, or
/// the call a definition is whatever it gives.
fn body_of<N>(definition: &Definition<N>) -> Option<&Expr<N>> {
    match &definition.kind {
        DefinitionKind::Function(function) | DefinitionKind::Constant(function) => {
            Some(&function.body)
        }
        DefinitionKind::Derived(call)
        | DefinitionKind::Generated(call)
        | DefinitionKind::Module(call) => Some(call),
        _ => None,
    }
}

/// What a tag is to everything after resolution: the type of its one
/// value, a tag told apart by the package's ID and where `symbol`
/// says it is declared.
pub(super) fn tag(package: &str, symbol: &Symbol) -> DefinitionKind<Reference> {
    DefinitionKind::Type(TypeDefinition {
        params: Vec::new(),
        ty: Type::Literal(crate::literal::Literal::Tag {
            package: package.to_owned(),
            tag: symbol.identity(),
        }),
    })
}

/// What a brand declared as `symbol` in the package of ID `package`
/// is once resolved: the type of what is minted under it.
pub(super) fn brand(package: &str, symbol: &Symbol) -> DefinitionKind<Reference> {
    DefinitionKind::Type(TypeDefinition {
        params: Vec::new(),
        ty: Type::Brand(crate::literal::Brand {
            package: package.to_owned(),
            brand: symbol.identity(),
        }),
    })
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
    /// The innermost expression the first error was at.
    blamed: Option<*const Expr>,
    /// The nodes as written that the resolved tree has no node for.
    dropped: Vec<*const Expr>,
    /// The nodes of the resolved tree that nothing wrote.
    added: Vec<*const Expr<Reference>>,
    /// The casts an error is not reported at, each with the value of
    /// the binding it is reported at instead.
    stood: Vec<(*const Expr<Reference>, *const Expr<Reference>)>,
    /// The members of the generated modules the definition names
    /// something through, as far as they have been asked for.
    given: Given,
    /// The generated modules whose members it needed and was not
    /// given.
    wanted: RefCell<Vec<ModuleId>>,
}

/// The members of generated modules, by module.
type Given = BTreeMap<ModuleId, Arc<Members>>;

impl<'a> Resolver<'a> {
    fn new(scope: &'a Scope, symbol: &'a Symbol, package: &'a str) -> Self {
        Self {
            scope,
            symbol,
            package,
            type_params: Vec::new(),
            locals: Vec::new(),
            next: 0,
            blamed: None,
            dropped: Vec::new(),
            added: Vec::new(),
            stood: Vec::new(),
            given: Given::new(),
            wanted: RefCell::default(),
        }
    }

    fn definition(&mut self, definition: &Definition) -> Result<Definition<Reference>, Error> {
        let kind = match &definition.kind {
            DefinitionKind::Type(definition) => {
                let params = self.type_params(&definition.params)?;
                DefinitionKind::Type(TypeDefinition {
                    params,
                    ty: self.ty(&definition.ty)?,
                })
            }
            DefinitionKind::Derived(call) => DefinitionKind::Derived(self.expr(call)?),
            DefinitionKind::Generated(call) => DefinitionKind::Generated(self.expr(call)?),
            DefinitionKind::Module(call) => DefinitionKind::Module(self.expr(call)?),
            DefinitionKind::Function(function) => {
                DefinitionKind::Function(self.function(function)?)
            }
            DefinitionKind::Constant(function) => {
                DefinitionKind::Constant(self.function(function)?)
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
            DefinitionKind::Tag => tag(self.package, self.symbol),
            // And a brand the type of what is minted under it.
            DefinitionKind::Brand => brand(self.package, self.symbol),
            DefinitionKind::Intrinsic => DefinitionKind::Intrinsic,
            DefinitionKind::Contract(key) => DefinitionKind::Contract(ContractDefinition {
                ty: self.ty(&key.ty)?,
            }),
            DefinitionKind::Extern(native) => {
                let target = |path| {
                    self.lookup(Namespace::Value, path)
                        .map(Reference::Definition)
                };
                DefinitionKind::Extern(Extern {
                    ability: self.ability(&native.ability)?,
                    row: self.row(&native.row)?,
                    targets: native
                        .targets
                        .iter()
                        .map(target)
                        .collect::<Result<_, _>>()?,
                    suspends: native.suspends,
                })
            }
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
                    tool: target.tool.as_ref().map(value).transpose()?,
                    bindings,
                })
            }
        };
        // A key is a tag, which is named as a type is.
        let mut annotations = Vec::new();
        for annotation in &definition.annotations {
            annotations.push(Annotation {
                key: Reference::Definition(self.lookup(Namespace::Type, &annotation.key)?),
                value: self.expr(&annotation.value)?,
            });
        }
        Ok(Definition {
            name: definition.name.clone(),
            public: definition.public,
            doc: definition.doc.clone(),
            kind,
            annotations,
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

    /// The abilities a row names. A module named in one stands for
    /// every ability of it that is public, so nothing after this knows
    /// a row by anything but its abilities.
    fn row(&self, row: &[Path]) -> Result<Vec<Reference>, Error> {
        let mut abilities = Vec::new();
        for path in row {
            let named = match (self.ability(path), path.0.as_slice()) {
                (Ok(ability), _) => vec![ability],
                (Err(unknown), [module]) => self.abilities_of(module).ok_or(unknown)?,
                (Err(unknown), _) => return Err(unknown),
            };
            for ability in named {
                if !abilities.contains(&ability) {
                    abilities.push(ability);
                }
            }
        }
        Ok(abilities)
    }

    /// Every public ability of the module imported as `alias`, if one
    /// is.
    fn abilities_of(&self, alias: &str) -> Option<Vec<Reference>> {
        let members = self.module(alias)?;
        let public = members
            .0
            .iter()
            .filter(|((namespace, _), member)| *namespace == Namespace::Ability && member.public);
        Some(
            public
                .map(|(_, member)| Reference::Definition(member.symbol.clone()))
                .collect(),
        )
    }

    fn bind(&mut self, name: &str) {
        self.locals.push((name.to_owned(), self.next));
        self.next += 1;
    }

    /// The call after `comptime`. An argument of it may name an
    /// ability, for a function that takes a reference to one.
    fn comptime(&mut self, call: &Expr) -> Result<Expr<Reference>, Error> {
        let Expr::Call { callee, args } = call else {
            return self.expr(call);
        };
        Ok(Expr::Call {
            callee: Box::new(self.expr(callee)?),
            args: args
                .iter()
                .map(|arg| self.named(arg))
                .collect::<Result<_, _>>()?,
        })
    }

    /// An argument of a comptime call: an expression, or the name of
    /// an ability or of a module this one imports, which are no
    /// expressions anywhere else. A name that is a value is that.
    fn named(&mut self, arg: &Expr) -> Result<Expr<Reference>, Error> {
        if let Expr::Name(path) = arg
            && let [alias] = path.0.as_slice()
            && self.name(path).is_err()
            && let Some(module) = self.imported(alias)
        {
            return Ok(Expr::Module(module));
        }
        if let Expr::Name(path) = arg
            && matches!(self.name(path), Err(Error::NotPerformed { .. }))
        {
            let ability = self.lookup(Namespace::Ability, path)?;
            return Ok(Expr::Name(Reference::Definition(ability)));
        }
        self.expr(arg)
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
            // The likeliest slip: an ability called without its mark.
            .map_err(|unknown| match self.lookup(Namespace::Ability, path) {
                Ok(_) => Error::NotPerformed {
                    symbol: self.symbol.clone(),
                    path: path.clone(),
                },
                Err(_) => unknown,
            })
    }
}
