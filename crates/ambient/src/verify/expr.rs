//! Expressions: the type of each node, from the node and what is inside
//! it.

use super::{Invalid, Verifier, shape};
use crate::checker::function_type;
use crate::core::{self, Expr, Primitive, Slot};
use crate::surface::{Key, Reference};
use crate::symbol::Symbol;
use crate::types::{Ty, TyKey, abilities, entry, generalize, substitute};

/// Abilities bound for part of a function: by its row and what it
/// captured, or by a provide.
pub(super) struct Layer {
    pub(super) abilities: Vec<Symbol>,
    /// Whether nothing bound outside this layer can be seen inside it.
    pub(super) restricted: bool,
}

/// A function's body, as far as it has been verified.
pub(super) struct Body<'v, 'a> {
    pub(super) verifier: &'v Verifier<'a>,
    /// The type of each value the function captured.
    captures: Vec<Ty>,
    /// The type of each local bound so far, by number: `None` once it is
    /// out of scope. A local's number is how many were bound before it,
    /// which is how lowering numbers them.
    pub(super) locals: Vec<Option<Ty>>,
    /// What binds abilities here, innermost last.
    pub(super) layers: Vec<Layer>,
    /// The groups this is inside of, innermost last: each as how many
    /// layers there were where it began.
    pub(super) groups: Vec<usize>,
}

impl Verifier<'_> {
    /// A function: its header is well-formed, and its body gives what
    /// the header says, using only what the header says it has.
    pub(super) fn function(mut self, function: &core::Function) -> Result<(), Invalid> {
        // A bound may mention any type parameter, itself included.
        self.env.bounds = vec![Ty::Unknown; function.type_params.len()];
        self.env.bounds = self.sound_all(&function.type_params)?;
        let params = self.sound_all(&function.params)?;
        let ret = self.sound(&function.ret)?;
        let mut bound = self.row(&function.row)?;
        bound.extend(self.row(&function.bindings)?);
        let mut body = Body {
            verifier: &self,
            captures: self.sound_all(&function.captures)?,
            locals: params.into_iter().map(Some).collect(),
            layers: vec![Layer {
                abilities: bound,
                restricted: false,
            }],
            groups: Vec::new(),
        };
        body.check(&function.body, &ret)
    }
}

impl Body<'_, '_> {
    /// Ends the scope of every local numbered `scope` or later.
    pub(super) fn leave(&mut self, scope: usize) {
        for local in &mut self.locals[scope..] {
            *local = None;
        }
    }

    pub(super) fn require(&self, ability: &Symbol) -> Result<(), Invalid> {
        for layer in self.layers.iter().rev() {
            if layer.abilities.contains(ability) {
                return Ok(());
            }
            if layer.restricted {
                break;
            }
        }
        Err(Invalid::Unbound(ability.to_string()))
    }

    pub(super) fn require_all(&self, row: &[Reference]) -> Result<(), Invalid> {
        abilities(row)
            .iter()
            .try_for_each(|ability| self.require(ability))
    }

    pub(super) fn check(&mut self, expr: &Expr, expected: &Ty) -> Result<(), Invalid> {
        let found = self.infer(expr)?;
        if self.verifier.env.subtype(&found, expected) {
            Ok(())
        } else {
            Err(self.verifier.mismatch(expected, &found))
        }
    }

    fn infer_all(&mut self, exprs: &[Expr]) -> Result<Vec<Ty>, Invalid> {
        exprs.iter().map(|expr| self.infer(expr)).collect()
    }

    /// A key of a record, as its type holds it.
    pub(super) fn key(&self, key: &core::Key) -> Result<TyKey, Invalid> {
        Ok(match key {
            core::Key::Field(name) => Key::Field(name.clone()),
            core::Key::Ability(slot) => {
                let named = self.verifier.symbol(*slot)?;
                let env = &self.verifier.env;
                if !env.abilities.contains_key(named) && !env.contracts.contains_key(named) {
                    return Err(Invalid::Slot);
                }
                Key::Ability(Reference::Definition(named.clone()))
            }
        })
    }

    /// The type of `expr`, in normal form.
    pub(super) fn infer(&mut self, expr: &Expr) -> Result<Ty, Invalid> {
        let verifier = self.verifier;
        let env = &verifier.env;
        Ok(match expr {
            Expr::Null => Ty::Null,
            Expr::Literal(literal) => Ty::Literal(literal.clone()),
            Expr::Local(number) => {
                let local = self.locals.get(*number).cloned().flatten();
                local.ok_or(Invalid::Local)?
            }
            Expr::Captured(position) => {
                let captured = self.captures.get(*position).cloned();
                captured.ok_or(Invalid::Local)?
            }
            Expr::Reference(slot, args) => self.function_value(*slot, args, &[], &[], 0)?,
            Expr::Closure {
                slot,
                type_args,
                captures,
                bindings,
            } => self.function_value(*slot, type_args, captures, bindings, 0)?,
            Expr::Call { callee, args } => {
                let found = self.infer(callee)?;
                let Ty::Function { params, ret, row } = env.unfold(&found) else {
                    return shape("something that is not a function is called");
                };
                if params.len() != args.len() {
                    return Err(Invalid::Arity);
                }
                self.require_all(&row)?;
                for (arg, param) in args.iter().zip(&params) {
                    self.check(arg, param)?;
                }
                *ret
            }
            Expr::Record(fields) => {
                let record = self.fields(fields)?;
                self.receives(&record)?;
                record
            }
            Expr::Tuple(members) => Ty::Tuple(self.infer_all(members)?.into()),
            Expr::List(members) => {
                let members = self.infer_all(members)?;
                Ty::List(Box::new(env.union(members)))
            }
            Expr::Field { target, key } => {
                let found = self.infer(target)?;
                let field = env.field(&found, &self.key(key)?);
                field.ok_or_else(|| Invalid::Shape("a field is read that may be missing".into()))?
            }
            Expr::Step(iterator) => self.step(iterator)?,
            Expr::Instantiate { function, types } => self.instantiate(function, types)?,
            Expr::Send { .. } => self.send(expr)?,
            Expr::Merge {
                left,
                right,
                keys,
                methods,
            } => self.merge(left, right, keys, methods)?,
            Expr::Let { value, body } => {
                let value = self.infer(value)?;
                let scope = self.locals.len();
                self.locals.push(Some(value));
                let ty = self.infer(body)?;
                self.leave(scope);
                ty
            }
            Expr::Primitive { operation, args } => self.primitive(*operation, args)?,
            Expr::Match { subject, arms } => self.matching(subject, arms)?,
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => {
                self.infer(value)?;
                let target = verifier.tested(ty)?;
                let alternative = self.infer(otherwise)?;
                env.union(vec![target, alternative])
            }
            Expr::Annotated { value, .. } => self.infer(value)?,
            Expr::Widen { value, ty } => {
                let seen = verifier.sound(ty)?;
                self.check(value, &seen)?;
                seen
            }
            // Whoever sent the tree is trusted with its brands
            // (`spec/exchange.md`): only what is under them is checked.
            Expr::Brand { value, ty } => {
                let branded = verifier.sound(ty)?;
                let Ty::Branded(under, _) = &branded else {
                    return Err(Invalid::Type);
                };
                self.check(value, under)?;
                branded
            }
            Expr::Cell { ty, value } => {
                let held = verifier.sound(ty)?;
                self.check(value, &held)?;
                Ty::Cell(Box::new(held))
            }
            _ => self.effect(expr)?,
        })
    }

    /// A function as a value: one that stands alone, or a closure over
    /// one that captures values and bindings. `pending` is how many of
    /// its last captures are not captured yet.
    fn function_value(
        &mut self,
        slot: Slot,
        type_args: &[core::Type],
        captures: &[Expr],
        bindings: &[Slot],
        pending: usize,
    ) -> Result<Ty, Invalid> {
        let verifier = self.verifier;
        let env = &verifier.env;
        let symbol = verifier.symbol(slot)?;
        let signature = env.functions.get(symbol).ok_or(Invalid::Slot)?;
        let held = verifier.held.get(symbol).ok_or(Invalid::Slot)?;
        if signature.native && !verifier.root {
            return shape("only a program's root names a native handler");
        }
        // Type arguments it is not given are parameters of its type: a
        // generic function named alone, or a generic lambda, which is
        // given only those of what it is written in.
        let given = self.verifier.sound_all(type_args)?;
        let Some(rest) = signature.type_params.len().checked_sub(given.len()) else {
            return Err(Invalid::Arity);
        };
        let base = env.bounds.len();
        let fresh = (base..base + rest).map(|at| Ty::Named(Reference::Parameter(at), Vec::new()));
        let args: Vec<Ty> = given.iter().cloned().chain(fresh).collect();
        for (arg, bound) in given.iter().zip(&signature.type_params) {
            let bound = env.normalize(&substitute(bound, &args));
            if !env.subtype(arg, &bound) {
                return Err(self.verifier.mismatch(&bound, arg));
            }
        }
        if captures.len() + pending != held.captures.len() {
            return Err(Invalid::Arity);
        }
        for (capture, ty) in captures.iter().zip(&held.captures) {
            self.check(capture, &env.normalize(&substitute(ty, &args)))?;
        }
        // A closure hands over the bindings its function captured, which
        // it must have itself.
        if verifier.row(bindings)? != held.bindings {
            return shape("a closure does not capture the bindings its function needs");
        }
        for ability in &held.bindings {
            self.require(ability)?;
        }
        let function = function_type(signature, &args);
        if rest == 0 {
            return Ok(env.normalize(&function));
        }
        let bounds = signature.type_params[given.len()..].iter();
        Ok(env.normalize(&Ty::Generic {
            bounds: bounds
                .map(|bound| generalize(&substitute(bound, &args), base, rest))
                .collect(),
            function: Box::new(generalize(&function, base, rest)),
        }))
    }

    /// The type of a record written out, before its methods are held
    /// to what they need: one written to be merged is part of a larger
    /// record.
    fn fields(&mut self, fields: &[(core::Key, Expr)]) -> Result<Ty, Invalid> {
        let mut types: Vec<(TyKey, Ty)> = Vec::new();
        for (key, value) in fields {
            let key = self.key(key)?;
            if entry(&types, &key).is_some() {
                return shape("a record has one key twice");
            }
            let ty = match value {
                Expr::Method(closure) => self.method(closure)?,
                value => self.infer(value)?,
            };
            if !self.verifier.env.holds(&key, &ty) {
                return shape("a record holds under a key what is not of the key's type");
            }
            types.push((key, ty));
        }
        types.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(Ty::Record(types))
    }

    /// A method of a record: a closure over the function that reads
    /// it, which captures the method's function and then the record.
    /// The record is not there until the field is read, and what the
    /// function takes it to be is what the method needs.
    fn method(&mut self, closure: &Expr) -> Result<Ty, Invalid> {
        let Expr::Closure {
            slot,
            type_args,
            captures,
            bindings,
        } = closure
        else {
            return shape("a method is not a closure");
        };
        let [function, Expr::Null] = captures.as_slice() else {
            return shape("a method does not capture a function and then nothing");
        };
        let captured = std::slice::from_ref(function);
        let read = self.function_value(*slot, type_args, captured, bindings, 1)?;
        let held = &self.verifier.held[self.verifier.symbol(*slot)?];
        let args = self.verifier.sound_all(type_args)?;
        let needs = held.captures.get(1).ok_or(Invalid::Arity)?;
        let needs = self.verifier.env.normalize(&substitute(needs, &args));
        Ok(Ty::Method {
            needs: Box::new(needs),
            function: Box::new(read),
        })
    }

    /// Fails unless a record has what each of its methods needs of it.
    fn receives(&self, record: &Ty) -> Result<(), Invalid> {
        let Ty::Record(fields) = record else {
            return Ok(());
        };
        for (_, ty) in fields {
            if let Ty::Method { needs, .. } = ty
                && !self.verifier.env.subtype(record, needs)
            {
                return Err(self.verifier.mismatch(needs, record));
            }
        }
        Ok(())
    }

    fn merge(
        &mut self,
        left: &Expr,
        right: &Expr,
        keys: &[core::Key],
        methods: &[core::Key],
    ) -> Result<Ty, Invalid> {
        let env = &self.verifier.env;
        // A record written here is held to what its methods need only
        // as part of the merged one.
        let mut operand = |expr: &Expr| match expr {
            Expr::Record(fields) => self.fields(fields),
            expr => self.infer(expr),
        };
        let left = operand(left)?;
        let right = operand(right)?;
        let (Ty::Record(mut fields), Ty::Record(over)) = (env.unfold(&left), env.unfold(&right))
        else {
            return shape("something that is not a record is merged");
        };
        // Exactly what the second record's type lists is taken from it.
        let taken: Result<Vec<TyKey>, Invalid> = keys.iter().map(|key| self.key(key)).collect();
        let listed: Vec<&TyKey> = env.ordered(&over).into_iter().map(|(key, _)| key).collect();
        if taken?.iter().ne(listed) {
            return shape("a merge does not take the keys its second record lists");
        }
        fields.retain(|(key, _)| entry(&over, key).is_none());
        fields.extend(over);
        fields.sort_by(|(a, _), (b, _)| a.cmp(b));
        // Each method the merged record lists receives it, and must be
        // listed, or it would stay bound to the record it had.
        let held: Result<Vec<TyKey>, Invalid> = methods.iter().map(|key| self.key(key)).collect();
        if held? != crate::types::methods(env, &fields) {
            return shape("a merge does not hand the merged record to the methods it lists");
        }
        let merged = Ty::Record(fields);
        self.receives(&merged)?;
        Ok(merged)
    }

    fn primitive(&mut self, operation: Primitive, args: &[Expr]) -> Result<Ty, Invalid> {
        match operation {
            Primitive::CellRead | Primitive::CellReplace | Primitive::CellWait => {
                return self.cell(operation, args);
            }
            _ => {}
        }
        let operands = self.infer_all(args)?;
        let given = self.verifier.env.operation(operation, &operands);
        given.map_or_else(|| shape("an operation is given what it does not take"), Ok)
    }

    /// An operation on a cell: the cell, then values it could hold.
    fn cell(&mut self, operation: Primitive, args: &[Expr]) -> Result<Ty, Invalid> {
        let values = match operation {
            Primitive::CellReplace => 2,
            Primitive::CellWait => 1,
            _ => 0,
        };
        let Some((cell, rest)) = args.split_first().filter(|(_, rest)| rest.len() == values) else {
            return Err(Invalid::Arity);
        };
        let found = self.infer(cell)?;
        let Ty::Cell(held) = self.verifier.env.unfold(&found) else {
            let expected = Ty::Cell(Box::new(Ty::Unknown));
            return Err(self.verifier.mismatch(&expected, &found));
        };
        for value in rest {
            self.check(value, &held)?;
        }
        Ok(match operation {
            Primitive::CellReplace => Ty::Bool,
            Primitive::CellWait => Ty::Null,
            _ => *held,
        })
    }
}
