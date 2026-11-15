//! Field access and operators.

use super::Checker;
use crate::core::{self, Primitive};
use crate::error::Error;
use crate::surface::{Expr, Operator, Pattern, Reference};
use crate::types::Ty;

impl Checker<'_> {
    /// `value` at type `ty` if the type describes it, and `otherwise` if
    /// not.
    pub(super) fn cast(
        &mut self,
        value: &Expr<Reference>,
        ty: &Ty,
        otherwise: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let (value, found) = self.infer(value)?;
        if !self.env.testable(ty) {
            return self.cast_near(value, &found, ty, otherwise);
        }
        self.tested(ty)?;
        let target = self.env.normalize(ty);
        let (otherwise, alternative) = self.infer(otherwise)?;
        let core = core::Expr::Cast {
            value: Box::new(value),
            ty: self.lower(&target),
            otherwise: Box::new(otherwise),
        };
        Ok((core, self.env.union(vec![target, alternative])))
    }

    /// A cast to a type no value can be tested for, which is one to the
    /// nearest type that can be where that tells as much
    /// ([`Self::asked`]). What it gives is then what the test lets
    /// through of the value's type, which a cast does not say: so it
    /// is a match, of one arm that binds what passes and one for
    /// `otherwise`.
    fn cast_near(
        &mut self,
        value: core::Expr,
        found: &Ty,
        ty: &Ty,
        otherwise: &Expr<Reference>,
    ) -> Result<(core::Expr, Ty), Error> {
        let asked = self.asked(found, &Pattern::Is(ty.clone()))?;
        let (Some(through), Pattern::Is(near)) = (self.env.narrow(found, &asked), &asked) else {
            return self.tested(ty).map(|()| (value, Ty::Never));
        };
        // The arm's binder is a local nothing in the text names.
        let frame = self.frame();
        let passed = frame.locals.len();
        frame.locals.insert(usize::MAX - passed, passed);
        let tested = core::Pattern::Is(self.lower(near));
        let (otherwise, alternative) = self.infer(otherwise)?;
        let core = core::Expr::Match {
            subject: Box::new(value),
            arms: vec![
                (
                    core::Pattern::Bind(Box::new(tested)),
                    core::Expr::Local(passed),
                ),
                (core::Pattern::Wildcard, otherwise),
            ],
        };
        Ok((core, self.env.union(vec![through, alternative])))
    }

    /// An operator applied to operands: the first operation it can mean
    /// that takes operands of their types.
    pub(super) fn operation(
        &mut self,
        operator: Operator,
        operands: &[Expr<Reference>],
    ) -> Result<(core::Expr, Ty), Error> {
        let (mut args, types) = self.infer_all(operands)?;
        // A step is not an operation: it may call the function that
        // steps the iterator.
        if operator == Operator::Step
            && let [iterator] = &types[..]
            && let Some(item) = self.env.items(iterator)
        {
            let iterator = Box::new(args.remove(0));
            return Ok((core::Expr::Step(iterator), self.env.step(&item)));
        }
        let meant = meanings(operator).iter().find_map(|operation| {
            let ty = self.env.operation(*operation, &types)?;
            Some((*operation, ty))
        });
        let Some((operation, ty)) = meant else {
            let found: Vec<String> = types.iter().map(|ty| self.env.show(ty)).collect();
            return Err(Error::Operands {
                symbol: self.symbol.clone(),
                operator: operator.written().to_owned(),
                found: found.join(", "),
            });
        };
        Ok((core::Expr::Primitive { operation, args }, ty))
    }
}

/// The operations an operator can mean, in the order they are tried.
fn meanings(operator: Operator) -> &'static [Primitive] {
    use Primitive as P;
    match operator {
        Operator::Add => &[P::IntAdd, P::FloatAdd],
        Operator::Subtract => &[P::IntSubtract, P::FloatSubtract],
        Operator::Multiply => &[P::IntMultiply, P::FloatMultiply],
        Operator::Divide => &[P::IntDivide, P::FloatDivide],
        Operator::Remainder => &[P::IntRemainder],
        Operator::Less => &[P::IntLess, P::FloatLess, P::StringLess],
        Operator::Equal => &[P::Equal],
        Operator::Length => &[P::StringLength, P::BytesLength, P::ListLength],
        Operator::Get => &[P::BytesGet, P::ListGet],
        Operator::Push => &[P::ListPush],
        Operator::Join => &[P::StringJoin, P::BytesJoin, P::ListJoin],
        Operator::Slice => &[P::StringSlice, P::BytesSlice, P::ListSlice],
        Operator::ToInt => &[P::FloatInt, P::StringInt],
        Operator::ToFloat => &[P::IntFloat, P::StringFloat],
        Operator::ToText => &[P::IntText, P::FloatText, P::BytesText],
        Operator::ToBytes => &[P::StringBytes, P::ListBytes],
        Operator::Code => &[P::StringCode],
        Operator::Character => &[P::CodeString],
        Operator::Iterator => &[P::IteratorNew],
        Operator::Iterate => &[P::StringIterate, P::ListIterate],
        Operator::Range => &[P::IntRange],
        Operator::Step => &[],
    }
}
