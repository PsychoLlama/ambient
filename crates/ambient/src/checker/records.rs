//! Records: making, reading, and merging them. A record keyed by
//! abilities is a bundle. See `docs/spec/handlers.md`.

use super::Checker;
use crate::core;
use crate::error::Error;
use crate::surface::{Expr, Key, Reference};
use crate::types::{Ty, TyKey, entry, methods};

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
        Some(info.handler(Ty::Unknown, Vec::new()))
    }

    /// Fails unless a value of type `ty` may be held under `key`.
    pub(super) fn held(&self, key: &TyKey, ty: &Ty) -> Result<(), Error> {
        if self.env.holds(key, ty) {
            return Ok(());
        }
        Err(Error::ContractType {
            symbol: self.symbol.clone(),
            name: Self::key_name(key),
            found: self.env.show(ty),
        })
    }

    /// A record written out, where one that holds `wanted` is expected:
    /// a field is held to what is wanted under its key.
    pub(super) fn record(
        &mut self,
        fields: &[(TyKey, Expr<Reference>)],
        wanted: &[(TyKey, Ty)],
    ) -> Result<(core::Expr, Ty), Error> {
        let (core, record) = self.fields(fields, &[], wanted)?;
        if let Ty::Record(types) = &record {
            self.receives(&record, types)?;
        }
        Ok((core, record))
    }

    /// A record written out, before its methods are held to what they
    /// need: one written to be merged is part of a larger record, which
    /// has the fields `under` besides.
    fn fields(
        &mut self,
        fields: &[(TyKey, Expr<Reference>)],
        under: &[(TyKey, Ty)],
        wanted: &[(TyKey, Ty)],
    ) -> Result<(core::Expr, Ty), Error> {
        let mut cores = Vec::new();
        let mut types: Vec<(TyKey, Ty)> = Vec::new();
        // What a method that does not say what it needs receives: the
        // record as a reader sees it, so far as that is known before
        // such a method's body is checked.
        let mut own: Vec<(TyKey, Ty)> = Vec::new();
        for (key, ty) in under {
            if fields.iter().all(|(written, _)| written != key) {
                own.push((key.clone(), ty.read()));
            }
        }
        for (key, value) in fields {
            if Self::implicit(value)
                && let Some(ty) = self.stated(value)?
            {
                own.push((key.clone(), ty));
            }
        }
        let mut waiting = Vec::new();
        for (key, value) in fields {
            if Self::implicit(value) {
                waiting.push((cores.len(), value));
                cores.push((self.key(key), core::Expr::Null));
                types.push((key.clone(), Ty::Never));
                continue;
            }
            // A method is read by its record, so nothing is wanted of
            // it but by the record's type as a whole.
            let wanted = entry(wanted, key).filter(|_| !matches!(value, Expr::Method(_)));
            let (core, ty) = match (self.expected_handler(key), wanted) {
                (Some(expected), _) => self.infer_as(value, &expected)?,
                (None, Some(wanted)) => (self.check(value, &wanted.read())?, wanted.read()),
                (None, None) => self.method(value, None)?,
            };
            own.push((key.clone(), ty.read()));
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
        own.sort_by(|(a, _), (b, _)| a.cmp(b));
        let own = Ty::Record(own);
        for (index, value) in waiting {
            (cores[index].1, types[index].1) = self.method(value, Some(&own))?;
        }
        for (key, ty) in &types {
            self.held(key, ty)?;
        }
        types.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok((core::Expr::Record(cores), Ty::Record(types)))
    }

    /// Reads what a record holds under `key`.
    pub(super) fn read(
        &mut self,
        target: &Expr<Reference>,
        key: &TyKey,
    ) -> Result<(core::Expr, Ty), Error> {
        let (core, ty) = self.infer(target)?;
        self.field(core, &ty, key)
    }

    /// Reads what `record`, of type `ty`, holds under `key`.
    pub(super) fn field(
        &mut self,
        core: core::Expr,
        ty: &Ty,
        key: &TyKey,
    ) -> Result<(core::Expr, Ty), Error> {
        let field = self.env.field(ty, key).ok_or_else(|| Error::NoField {
            symbol: self.symbol.clone(),
            ty: self.env.show(ty),
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
        // A record written here is whole only once merged, so its
        // methods are held to the merged record alone.
        let (core_left, left) = match left {
            Expr::Record(fields) => self.fields(fields, &[], &[])?,
            left => self.infer(left)?,
        };
        let below = self.env.unfold(&left);
        let (core_right, right) = match (right, &below) {
            (Expr::Record(fields), Ty::Record(below)) => self.fields(fields, below, &[])?,
            (Expr::Record(fields), _) => self.fields(fields, &[], &[])?,
            (right, _) => self.infer(right)?,
        };
        let (Ty::Record(mut fields), Ty::Record(over)) = (below, self.env.unfold(&right)) else {
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                operator: "...".to_owned(),
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
        // Each method the operands list receives the merged record.
        let merged = Ty::Record(fields);
        let Ty::Record(fields) = &merged else {
            return Ok((core::Expr::Null, merged));
        };
        self.receives(&merged, fields)?;
        let methods = methods(self.env, fields);
        let methods = methods.iter().map(|key| self.key(key)).collect();
        let core = core::Expr::Merge {
            left: Box::new(core_left),
            right: Box::new(core_right),
            keys,
            methods,
        };
        Ok((core, merged))
    }
}
