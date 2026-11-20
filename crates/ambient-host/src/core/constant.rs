//! What a constant's value may be written as. See "Values" in
//! `docs/spec/constants.md`.
//!
//! This is the one rule for it: checking holds a written constant to
//! it, the core checker a received one, and what lays a program out
//! reads a value by these forms and no other.

use super::{Expr, Key, Slot};
use crate::value::Value;

impl Expr {
    /// Whether the expression is a value form: what is a value as it
    /// stands, with nothing to call, perform, or test.
    ///
    /// A function is one if it holds nothing of where it was made: a
    /// definition named, or a lambda that captured no binding and no
    /// value but another value form. A method's closure is such a
    /// lambda, over its function and the record it has yet to be
    /// read from.
    #[must_use]
    pub fn is_value_form(&self) -> bool {
        let all = |members: &[Self]| members.iter().all(Self::is_value_form);
        match self {
            Self::Null | Self::Literal(_) | Self::Constant(_) | Self::Reference(..) => true,
            Self::Tuple(members) | Self::List(members) => all(members),
            Self::Record(fields) => fields.iter().all(|(_, value)| value.is_value_form()),
            Self::Method(closure) => {
                matches!(&**closure, Self::Closure { .. }) && closure.is_value_form()
            }
            Self::Closure {
                captures, bindings, ..
            } => bindings.is_empty() && all(captures),
            // A generic function at type arguments, named outright.
            Self::Instantiate { function, .. } => {
                matches!(&**function, Self::Reference(_, types) if types.is_empty())
            }
            Self::Widen { value, .. }
            | Self::Brand { value, .. }
            | Self::Annotated { value, .. }
            | Self::At(_, value) => value.is_value_form(),
            _ => false,
        }
    }

    /// The value a value form is, if it is plain data: one that holds
    /// no function, and no record that holds anything under an
    /// ability. `constant` gives what another constant holds.
    #[must_use]
    pub fn data(&self, constant: &dyn Fn(Slot) -> Option<Value>) -> Option<Value> {
        let data = |value: &Self| value.data(constant);
        let all = |members: &[Self]| members.iter().map(data).collect::<Option<Vec<_>>>();
        Some(match self {
            Self::Null => Value::Null,
            Self::Literal(literal) => literal.value(),
            Self::Constant(slot) => constant(*slot)?,
            Self::Tuple(members) => Value::Tuple(all(members)?),
            Self::List(members) => Value::List(all(members)?),
            Self::Record(fields) => {
                let field = |(key, value): &(Key, Self)| match key {
                    Key::Field(name) => Some((name.clone(), data(value)?)),
                    Key::Ability(_) => None,
                };
                Value::Record(fields.iter().map(field).collect::<Option<_>>()?)
            }
            Self::Widen { value, .. }
            | Self::Brand { value, .. }
            | Self::Annotated { value, .. }
            | Self::At(_, value) => data(value)?,
            _ => return None,
        })
    }

    /// Every slot a value form names: of a definition, a key, or a
    /// type.
    #[must_use]
    pub fn value_slots(&self) -> Vec<Slot> {
        let mut slots = Vec::new();
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            let types: &[super::Type] = match value {
                Self::Constant(slot) => {
                    slots.push(*slot);
                    &[]
                }
                Self::Reference(slot, types) => {
                    slots.push(*slot);
                    types
                }
                Self::Closure {
                    slot, type_args, ..
                } => {
                    slots.push(*slot);
                    type_args
                }
                Self::Instantiate { types, .. } => types,
                Self::Record(fields) => {
                    slots.extend(fields.iter().filter_map(|(key, _)| match key {
                        super::Key::Ability(slot) => Some(*slot),
                        super::Key::Field(_) => None,
                    }));
                    &[]
                }
                _ => &[],
            };
            slots.extend(types.iter().flat_map(super::Type::slots));
            value.children(&mut |child| pending.push(child));
        }
        slots
    }

    /// The constants a value form holds as they stand, so that making
    /// this value reads each. One named inside a function is not met:
    /// a lambda is a definition of its own.
    pub fn constants(&self, visit: &mut impl FnMut(Slot)) {
        if let Self::Constant(slot) = self {
            visit(*slot);
        }
        self.children(&mut |child| child.constants(visit));
    }
}

#[cfg(test)]
mod tests {
    use super::{Expr, Slot};
    use crate::literal::Literal;
    use crate::value::Value;

    fn int(value: i64) -> Expr {
        Expr::Literal(Literal::Int(value))
    }

    fn closure(bindings: Vec<Slot>) -> Expr {
        Expr::Closure {
            slot: Slot(0),
            type_args: Vec::new(),
            captures: Vec::new(),
            bindings,
        }
    }

    #[test]
    fn data_and_functions_are_value_forms() {
        let list = Expr::List(vec![closure(Vec::new())]);
        let value = Expr::Tuple(vec![int(1), list, Expr::Constant(Slot(1))]);
        assert!(value.is_value_form());
    }

    #[test]
    fn a_call_is_no_value_form() {
        let call = Expr::Call {
            callee: Box::new(Expr::Reference(Slot(0), Vec::new())),
            args: Vec::new(),
        };
        assert!(!Expr::List(vec![int(1), call]).is_value_form());
    }

    #[test]
    fn a_closure_that_captured_a_binding_is_no_value_form() {
        assert!(!closure(vec![Slot(1)]).is_value_form());
    }

    #[test]
    fn the_constants_a_value_holds_are_read() {
        let value = Expr::Tuple(vec![Expr::Constant(Slot(2)), int(1)]);
        let mut read = Vec::new();
        value.constants(&mut |slot| read.push(slot));
        assert_eq!(read, [Slot(2)]);
    }

    #[test]
    fn plain_data_is_read_as_its_value_through_the_constants_it_names() {
        let pair = Expr::Tuple(vec![int(1), Expr::Constant(Slot(0))]);
        let held = |_| Some(Value::Int(2));
        let expected = Value::Tuple(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(pair.data(&held), Some(expected));
    }

    #[test]
    fn a_value_that_holds_a_function_is_not_plain_data() {
        let pair = Expr::Tuple(vec![int(1), closure(Vec::new())]);
        assert_eq!(pair.data(&|_| None), None);
    }
}
