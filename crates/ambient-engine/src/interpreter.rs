use crate::{
    syntax::{Expression, Literal},
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
struct Interpreter {
    resources: HashMap<blake3::Hash, Expression>,
}

impl Interpreter {
    pub(self) fn eval_literal_expr(&self, node: &Literal) -> Result<Value, RuntimeError> {
        Ok(match node {
            Literal::Boolean(value) => Value::Boolean(*value),
            Literal::Int32(value) => Value::Int32(*value),
            Literal::Hash(hash) => Value::Reference(hash.clone()),
        })
    }

    pub(self) fn eval_expr(&self, node: &Expression) -> Result<Value, RuntimeError> {
        match node {
            // `#abc()`
            Expression::FunctionCall { callee, arguments } => {
                let Value::Reference(hash) = **callee else {
                    return Err(RuntimeError::UnsupportedCallTarget(
                        "Not a hash literal.".to_string(),
                    ));
                };

                self.eval_function_call(&hash, arguments)
            }

            // `123`
            Expression::Literal(literal) => self.eval_literal_expr(literal),

            _ => todo!(),
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
            Expression::FunctionDefinition { body, .. } => self.eval_expr(body),
            _ => Err(RuntimeError::UnsupportedCallValue(hash.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_evaluate_literal_hash() {
        let hash = blake3::hash(b"id");
        let node = Literal::Hash(hash);
        let result = Interpreter::default().eval_literal_expr(&node);
        assert_eq!(result, Ok(Value::Reference(hash)));
    }

    #[test]
    fn test_eval_function_call() {
        let hash = blake3::hash(b"id");

        // Define a function that always returns `false`.
        let interpreter = Interpreter {
            resources: HashMap::from_iter(vec![(
                hash,
                Expression::FunctionDefinition {
                    parameters: vec![],
                    body: Box::new(Expression::Literal(Literal::Boolean(false))),
                },
            )]),
        };

        let result = interpreter.eval_expr(&Expression::FunctionCall {
            callee: Box::new(Value::Reference(hash.clone())),
            arguments: vec![],
        });

        assert_eq!(result, Ok(Value::Boolean(false)));
    }
}
