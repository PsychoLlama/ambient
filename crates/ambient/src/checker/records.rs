//! Records: making, reading, and merging them. A record keyed by
//! abilities is a bundle. See `spec/handlers.md`.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Key, Reference};
use crate::types::{Ty, TyKey, entry};

impl Checker<'_> {
    /// A key as a core tree holds it.
    pub(super) fn key(&mut self, key: &TyKey) -> core::Key {
        let env = self.env;
        env.lower_key(key, &mut |symbol| self.slot(symbol))
    }

    pub(super) fn key_name(key: &TyKey) -> String {
        match key {
            Key::Field(name) => name.clone(),
            Key::Ability(Reference::Definition(ability)) => format!("[{}]", ability.name),
            Key::Ability(_) => "[?]".to_owned(),
        }
    }

    /// What a handler written under `key` is expected to take, if `key`
    /// is an ability: enough to type the parameters of a lambda.
    fn expected_handler(&self, key: &TyKey) -> Option<Ty> {
        let Key::Ability(Reference::Definition(ability)) = key else {
            return None;
        };
        let info = self.env.abilities.get(ability)?;
        Some(Ty::Function {
            params: info.params.clone(),
            ret: Box::new(Ty::Unknown),
            row: Vec::new(),
        })
    }

    pub(super) fn record(
        &mut self,
        fields: &[(TyKey, Expr<Reference>)],
    ) -> Result<(core::Expr, Ty), Error> {
        let mut cores = Vec::new();
        let mut types: Vec<(TyKey, Ty)> = Vec::new();
        for (key, value) in fields {
            let (core, ty) = match (self.expected_handler(key), value) {
                (Some(expected), _) => self.infer_as(value, &expected)?,
                (None, Expr::Method(function)) => self.method(function)?,
                (None, _) => self.infer(value)?,
            };
            // Two paths can name one ability, which a builder cannot see.
            if let (Some(_), Key::Ability(Reference::Definition(ability))) =
                (entry(&types, key), key)
            {
                return Err(Error::DuplicateHandler {
                    symbol: self.symbol.clone(),
                    ability: ability.clone(),
                });
            }
            cores.push((self.key(key), core));
            types.push((key.clone(), ty));
        }
        types.sort_by(|(a, _), (b, _)| a.cmp(b));
        let record = Ty::Record(types);
        if let Ty::Record(types) = &record {
            self.receives(&record, types)?;
        }
        Ok((core::Expr::Record(cores), record))
    }

    /// Reads what a record holds under `key`.
    pub(super) fn read(
        &mut self,
        target: &Expr<Reference>,
        key: &TyKey,
    ) -> Result<(core::Expr, Ty), Error> {
        let (core, ty) = self.infer(target)?;
        let field = self.env.field(&ty, key).ok_or_else(|| Error::NoField {
            symbol: self.symbol.clone(),
            ty: self.env.show(&ty),
            name: Self::key_name(key),
        })?;
        let core = core::Expr::Field {
            target: Box::new(core),
            key: self.key(key),
        };
        Ok((core, field))
    }

    /// A record with the fields of both, `right`'s where both have one.
    ///
    /// Only what `right`'s type says it holds is taken from it. A record
    /// may hold more than its type says, and a field nothing knows the
    /// type of must not replace one of `left`'s.
    pub(super) fn merge(
        &mut self,
        left: &Expr<Reference>,
        right: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (core_left, left) = self.infer(left)?;
        let (core_right, right) = self.infer(right)?;
        let (Ty::Record(mut fields), Ty::Record(over)) =
            (self.env.unfold(&left), self.env.unfold(&right))
        else {
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                found: format!("{}, {}", self.env.show(&left), self.env.show(&right)),
            });
        };
        let keys = self
            .env
            .ordered(&over)
            .into_iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let keys = keys.iter().map(|key| self.key(key)).collect();
        fields.retain(|(key, _)| entry(&over, key).is_none());
        fields.extend(over);
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        // TODO: a method the operands list should receive the merged
        // record, checked against it here. Until then every method
        // stays bound to the record it had.
        for (_, ty) in &mut fields {
            *ty = ty.read();
        }
        let core = core::Expr::Merge {
            left: Box::new(core_left),
            right: Box::new(core_right),
            keys,
        };
        Ok((core, Ty::Record(fields)))
    }
}
