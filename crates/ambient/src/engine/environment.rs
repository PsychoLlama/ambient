//! What checking a definition reads about others, and what covers it in
//! the check key. See `docs/spec/definitions.md` and `docs/spec/methods.md`.
//!
//! Most of it follows from the names in the definition's tree. A method
//! call is the exception: what it means waits for the type of its
//! receiver, so the function it means for each built-in shape is
//! gathered here.

use ambient_runtime::FAULT_PACKAGE;

use super::Engine;
use super::check::definitions;
use crate::error::Error;
use crate::hash::{Hasher, encode};
use crate::surface::{DefinitionKind, ExternKind, Param, Reference, Shape};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::types::{Env, Signature, Ty, abilities};

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
                Namespace::Type | Namespace::Ability | Namespace::Contract => {
                    key.hash(&self.hash(&used)?);
                    roots.push(used);
                }
                Namespace::Value => {
                    key.hash(&self.interface(&used)?.digest);
                    if let Some((ty, named)) = self.constant_type(&used)? {
                        roots.extend(named);
                        env.constants.insert(used, ty);
                    } else if let Some((signature, named)) = self.signature(&used)? {
                        roots.extend(named);
                        env.functions.insert(used, signature);
                    }
                }
            }
        }
        roots.extend(self.methods(sends, &mut env, key)?);
        self.gather(roots, &mut env)?;
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
        let constants = std::mem::take(&mut env.constants);
        let normal = constants.into_iter();
        env.constants = normal
            .map(|(symbol, ty)| (symbol, env.normalize(&ty)))
            .collect();
        Ok(env)
    }

    /// The type a constant states, and the definitions it names.
    /// Nothing, for a definition that is no constant.
    pub(super) fn constant_type(
        &self,
        symbol: &Symbol,
    ) -> Result<Option<(Ty, Vec<Symbol>)>, Error> {
        if !symbol.lambda.is_empty() || self.received(symbol).is_some() {
            return Ok(None);
        }
        let resolved = self.resolve(symbol)?;
        let DefinitionKind::Constant(constant) = &resolved.definition.kind else {
            return Ok(None);
        };
        let named = definitions(|visit| constant.header_names(&mut |name| visit(name)));
        Ok(Some((constant.ret.clone(), named)))
    }

    /// Adds to `env` the function each method called one of `sends`
    /// means for each built-in shape: the public function of that name
    /// in the shape's module of the standard library. Gives the
    /// definitions their headers name.
    fn methods(
        &self,
        sends: &[String],
        env: &mut Env,
        key: &mut Hasher,
    ) -> Result<Vec<Symbol>, Error> {
        let mut named = Vec::new();
        for shape in Shape::ALL {
            let module = ModuleId::new(FAULT_PACKAGE, shape.module());
            if self.sources.module(&module).is_none() {
                continue;
            }
            let members = self.members(&module)?;
            for name in sends {
                let member = members.0.get(&(Namespace::Value, name.clone()));
                let Some(member) = member.filter(|member| member.public) else {
                    continue;
                };
                let Some((signature, mentions)) = self.signature(&member.symbol)? else {
                    continue;
                };
                // Which function it is, and what calling it reads.
                key.bytes(&encode(&member.symbol));
                key.hash(&self.interface(&member.symbol)?.digest);
                named.extend(mentions);
                let function = member.symbol.clone();
                env.functions.entry(function.clone()).or_insert(signature);
                env.methods.insert((shape, name.clone()), function);
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
        // A generated function has the header its call gave.
        let written = self.function_of(symbol, &resolved.definition)?;
        let written = written.as_deref();
        let named = definitions(|visit| match written {
            Some(header) => header.header_names(&mut |name| visit(name)),
            None => resolved.definition.names(&mut |name| visit(name)),
        });
        let types = |params: &[Param<Reference>]| params.iter().map(|p| p.ty.clone()).collect();
        let row = |row: &[Reference]| {
            let mut row = abilities(row);
            row.sort();
            row.dedup();
            row
        };
        let signature = match (&resolved.definition.kind, written) {
            (_, Some(header)) => Signature {
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
            (DefinitionKind::Extern(native), _) => match &native.kind {
                ExternKind::Function {
                    params,
                    ret,
                    row: takes,
                } => Signature {
                    type_params: Vec::new(),
                    params: types(params),
                    ret: ret.clone(),
                    row: row(takes),
                    native: false,
                },
                // A native handler takes what its ability takes.
                ExternKind::Handler {
                    ability,
                    row: takes,
                    ..
                } => {
                    let Reference::Definition(ability) = ability else {
                        return Ok(None);
                    };
                    let ability = self.resolve(ability)?;
                    let DefinitionKind::Ability(ability) = &ability.definition.kind else {
                        return Ok(None);
                    };
                    let bounds = ability.type_params.iter();
                    let mut params: Vec<Ty> = types(&ability.params);
                    // One that does not return is handed the trace of
                    // the abort it ends: see `checker::world`.
                    if ability.ret == Ty::Never {
                        params.push(crate::types::trace());
                    }
                    Signature {
                        type_params: bounds
                            .map(|p| p.bound.clone().unwrap_or(Ty::Unknown))
                            .collect(),
                        params,
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
