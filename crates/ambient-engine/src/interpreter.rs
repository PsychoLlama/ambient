use crate::{
    content_hash::ContentHash,
    syntax::{Expression, Literal, Resource},
    value::Value,
};

use std::collections::HashMap;

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("Unknown hash: {0}")]
    UnknownHash(blake3::Hash),

    /// Tried to call something that isn't a hash reference.
    #[error("Unsupported call target: {0}")]
    UnsupportedCallTarget(String),

    /// Hash reference resolved to something that isn't a function.
    #[error("{0:?} is not a hash")]
    UnsupportedCallValue(blake3::Hash),
}

#[derive(Default)]
pub struct Interpreter {
    /// Globally shared resources, such as constants and functions. These are content addressed and
    /// can be shared over a network.
    resources: HashMap<blake3::Hash, Resource>,
}

impl Interpreter {
    pub fn with_resources(resources: Vec<Resource>) -> Self {
        Self {
            // Hash all resources and index them in the resource hashmap.
            resources: resources
                .into_iter()
                .map(|resource| (resource.hash(), resource))
                .collect(),
        }
    }

    pub(self) fn eval_literal_expr(&self, node: &Literal) -> Result<Value, RuntimeError> {
        Ok(match node {
            Literal::Boolean(value) => Value::Boolean(*value),
            Literal::Int32(value) => Value::Int32(*value),
            Literal::Hash(hash) => {
                let Some(resource) = self.resources.get(hash) else {
                    return Err(RuntimeError::UnknownHash(hash.clone()));
                };

                match resource {
                    // Don't invoke the function, just return a reference.
                    Resource::FunctionDefinition { .. } => Value::Reference(hash.clone()),

                    // Treat it like a variable. Resolve the value.
                    Resource::Const(literal) => self.eval_literal_expr(literal)?,
                }
            }
        })
    }

    pub fn eval_expr(&self, node: &Expression) -> Result<Value, RuntimeError> {
        match node {
            // `#abc()`
            Expression::FunctionCall { callee, arguments } => {
                let Expression::Literal(Literal::Hash(hash)) = **callee else {
                    return Err(RuntimeError::UnsupportedCallTarget(
                        "Not a hash literal.".to_string(),
                    ));
                };

                self.eval_function_call(&hash, arguments)
            }

            // `123`
            Expression::Literal(literal) => self.eval_literal_expr(literal),
        }
    }

    pub(self) fn eval_function_call(
        &self,
        hash: &blake3::Hash,
        _args: &Vec<Expression>,
    ) -> Result<Value, RuntimeError> {
        let resource = self
            .resources
            .get(&hash)
            .ok_or(RuntimeError::UnknownHash(hash.clone()))?;

        match resource {
            // TODO: Support parameters.
            Resource::FunctionDefinition { body, .. } => self.eval_expr(body),
            _ => Err(RuntimeError::UnsupportedCallValue(hash.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash::ContentHash;

    #[test]
    fn test_evaluate_literal_boolean() {
        let node = Literal::Boolean(true);
        let result = Interpreter::default().eval_literal_expr(&node);

        assert_eq!(result, Ok(Value::Boolean(true)));
    }

    #[test]
    fn test_evaluate_literal_i32() {
        let node = Literal::Int32(42);
        let result = Interpreter::default().eval_literal_expr(&node);

        assert_eq!(result, Ok(Value::Int32(42)));
    }

    #[test]
    fn test_evaluate_unknown_reference_fails() {
        let hash = blake3::hash(b"no-such-id");
        let node = Literal::Hash(hash);
        let result = Interpreter::default().eval_literal_expr(&node);

        assert_eq!(result, Err(RuntimeError::UnknownHash(hash)));
    }

    #[test]
    fn test_resolve_constant_value_from_hash() {
        let value = Resource::Const(Literal::Int32(1234));
        let hash = value.hash();

        let interpreter = Interpreter::with_resources(vec![value]);
        let result = interpreter.eval_literal_expr(&Literal::Hash(hash));

        assert_eq!(result, Ok(Value::Int32(1234)));
    }

    #[test]
    fn test_recursive_resolve_constant_value_from_hash() {
        let value = Resource::Const(Literal::Int32(1234));
        let hash = value.hash();

        let reference = Resource::Const(Literal::Hash(hash));
        let reference_hash = reference.hash();

        let interpreter = Interpreter::with_resources(vec![value, reference]);
        let result = interpreter.eval_literal_expr(&Literal::Hash(reference_hash));

        assert_eq!(result, Ok(Value::Int32(1234)));
    }

    #[test]
    fn test_resolve_function_ref_from_hash() {
        let value = Resource::FunctionDefinition {
            body: Box::new(Expression::Literal(Literal::Boolean(true))),
        };

        let hash = value.hash();
        let interpreter = Interpreter::with_resources(vec![value]);
        let result = interpreter.eval_literal_expr(&Literal::Hash(hash));

        // Function is not called. It only returns a reference.
        assert_eq!(result, Ok(Value::Reference(hash)));
    }

    #[test]
    fn test_eval_function_call() {
        let func = Resource::FunctionDefinition {
            body: Box::new(Expression::Literal(Literal::Boolean(false))),
        };

        let hash = func.hash();
        let interpreter = Interpreter::with_resources(vec![func]);
        let result = interpreter.eval_expr(&Expression::FunctionCall {
            callee: Box::new(Expression::Literal(Literal::Hash(hash))),
            arguments: vec![],
        });

        assert_eq!(result, Ok(Value::Boolean(false)));
    }

    #[test]
    fn test_eval_function_call_two_layers_deep() {
        let fn1 = Resource::FunctionDefinition {
            body: Box::new(Expression::Literal(Literal::Boolean(true))),
        };

        let fn2 = Resource::FunctionDefinition {
            body: Box::new(Expression::FunctionCall {
                callee: Box::new(Expression::Literal(Literal::Hash(fn1.hash()))),
                arguments: vec![],
            }),
        };

        let expr = Expression::FunctionCall {
            callee: Box::new(Expression::Literal(Literal::Hash(fn2.hash()))),
            arguments: vec![],
        };

        let interpreter = Interpreter::with_resources(vec![fn1, fn2]);
        let result = interpreter.eval_expr(&expr);
        assert_eq!(result, Ok(Value::Boolean(true)));
    }
}
