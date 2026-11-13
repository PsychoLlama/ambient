//! What checking a definition reads about others, and what covers it in
//! the check key. See `spec/definitions.md` and `spec/methods.md`.
//!
//! Most of it follows from the names in the definition's tree. A method
//! call is the exception: what it means waits for the type of its
//! receiver, so every function it could mean is gathered here, for each
//! built-in shape and each type definition the checker can meet.

use ambient_runtime::FAULT_PACKAGE;

use super::Engine;
use super::check::definitions;
use crate::error::Error;
use crate::hash::{Hasher, encode};
use crate::surface::{DefinitionKind, ExternKind, Param, Reference, Shape};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::types::{Env, Receiver, Signature, Ty, abilities};

impl Engine<'_> {
    /// Everything checking reads about the definitions in `used` and
    /// about the methods called `sends`, with what covers it added to
    /// `key`.
    pub(super) fn environment(
        &self,
        used: Vec<Symbol>,
        sends: &[String],
        key: &mut Hasher,
    ) -> Result<Env, Error> {
        let mut env = Env::default();
        let mut roots = Vec::new();
        for used in used {
            match used.namespace {
                Namespace::Type | Namespace::Ability | Namespace::Key => {
                    key.hash(&self.hash(&used)?);
                    roots.push(used);
                }
                Namespace::Value => {
                    key.hash(&self.interface(&used)?.digest);
                    if let Some((signature, named)) = self.signature(&used)? {
                        roots.extend(named);
                        env.functions.insert(used, signature);
                    }
                }
            }
        }
        // A method's header may name a type whose module has methods
        // too, so this goes round until nothing new is named.
        loop {
            self.gather(roots, &mut env)?;
            roots = self.methods(sends, &mut env, key)?;
            if roots.is_empty() {
                break;
            }
        }
        // Headers are read in normal form everywhere.
        let functions = std::mem::take(&mut env.functions);
        env.functions = functions
            .into_iter()
            .map(|(symbol, signature)| {
                let signature = Signature {
                    type_params: signature
                        .type_params
                        .iter()
                        .map(|bound| env.normalize(bound))
                        .collect(),
                    params: signature.params.iter().map(|p| env.normalize(p)).collect(),
                    ret: env.normalize(&signature.ret),
                    ..signature
                };
                (symbol, signature)
            })
            .collect();
        Ok(env)
    }

    /// Adds to `env` each function a method called one of `sends` could
    /// mean that it does not have yet: a public function of that name
    /// in a shape's module of the standard library, or in the module
    /// of a type definition `env` holds. Gives the definitions their
    /// headers name.
    fn methods(
        &self,
        sends: &[String],
        env: &mut Env,
        key: &mut Hasher,
    ) -> Result<Vec<Symbol>, Error> {
        let shapes = Shape::ALL.into_iter().map(|shape| {
            let module = ModuleId::new(FAULT_PACKAGE, shape.module());
            (Receiver::Shape(shape), module)
        });
        // Only a recursive definition is still a name once normalized.
        let types = env.types.iter().filter(|(_, info)| info.recursive);
        let types =
            types.map(|(symbol, _)| (Receiver::Type(symbol.clone()), symbol.module.clone()));
        let receivers: Vec<(Receiver, ModuleId)> = shapes.chain(types).collect();

        let mut named = Vec::new();
        for (receiver, module) in receivers {
            if self.sources.module(&module).is_none() {
                continue;
            }
            let members = self.members(&module)?;
            for name in sends {
                let at = (receiver.clone(), name.clone());
                let member = members.0.get(&(Namespace::Value, name.clone()));
                let Some(member) = member.filter(|member| member.public) else {
                    continue;
                };
                if env.methods.contains_key(&at) {
                    continue;
                }
                let Some((signature, mentions)) = self.signature(&member.symbol)? else {
                    continue;
                };
                // Which function it is, and what calling it reads.
                key.bytes(&encode(&member.symbol));
                key.hash(&self.interface(&member.symbol)?.digest);
                named.extend(mentions);
                env.functions
                    .entry(member.symbol.clone())
                    .or_insert(signature);
                env.methods.insert(at, member.symbol.clone());
            }
        }
        Ok(named)
    }

    /// The header of a function or an extern as written, and the
    /// definitions it names. A target has none: it is not a value.
    pub(super) fn signature(
        &self,
        symbol: &Symbol,
    ) -> Result<Option<(Signature, Vec<Symbol>)>, Error> {
        let resolved = self.resolve(symbol)?;
        let named = definitions(|visit| match &resolved.definition.kind {
            DefinitionKind::Function(header) => header.header_names(&mut |name| visit(name)),
            _ => resolved.definition.names(&mut |name| visit(name)),
        });
        let types = |params: &[Param<Reference>]| params.iter().map(|p| p.ty.clone()).collect();
        let row = |row: &[Reference]| {
            let mut row = abilities(row);
            row.sort();
            row.dedup();
            row
        };
        let signature = match &resolved.definition.kind {
            DefinitionKind::Function(header) => Signature {
                type_params: header
                    .type_params
                    .iter()
                    .map(|p| p.bound.clone().unwrap_or(Ty::Unknown))
                    .collect(),
                params: types(&header.params),
                ret: header.ret.clone(),
                row: row(&header.row),
                native: false,
            },
            DefinitionKind::Extern(native) => match &native.kind {
                ExternKind::Function { params, ret } => Signature {
                    type_params: Vec::new(),
                    params: types(params),
                    ret: ret.clone(),
                    row: Vec::new(),
                    native: false,
                },
                // A native handler takes what its ability takes.
                ExternKind::Handler {
                    ability,
                    row: takes,
                } => {
                    let Reference::Definition(ability) = ability else {
                        return Ok(None);
                    };
                    let ability = self.resolve(ability)?;
                    let DefinitionKind::Ability(ability) = &ability.definition.kind else {
                        return Ok(None);
                    };
                    Signature {
                        type_params: Vec::new(),
                        params: types(&ability.params),
                        ret: ability.ret.clone(),
                        row: row(takes),
                        native: true,
                    }
                }
            },
            _ => return Ok(None),
        };
        Ok(Some((signature, named)))
    }
}
