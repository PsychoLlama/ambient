#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]

use crate::{
    content_hash::ContentHash,
    syntax::{Expression, Literal, Resource},
    value::Value,
};

use std::collections::HashMap;

#[derive(Default)]
pub struct Interpreter {
    /// Globally shared resources, such as constants and functions. These are content addressed and
    /// can be shared over a network.
    resources: HashMap<blake3::Hash, Resource>,
}

impl Interpreter {
    /// Instantiate an interpreter with a list of resources (functions, constants).
    pub fn with_resources(resources: Vec<Resource>) -> Self {
        Self {
            // Hash all resources and index them in the resource hashmap.
            resources: resources
                .into_iter()
                .map(|resource| (resource.hash(), resource))
                .collect(),
        }
    }

    /// Run a function by its hash passing arguments along.
    pub fn call(&self, func_id: &blake3::Hash) -> Result<Value, RuntimeError> {
        let ctx = EngineContext::default();
        self.eval_function_call(&ctx, func_id)
    }

    /// Evaluate an arbitrary expression.
    pub fn eval(&self, node: &Expression) -> Result<Value, RuntimeError> {
        let ctx = EngineContext::default();
        self.eval_expr(&ctx, node)
    }

    fn eval_literal_expr(
        &self,
        ctx: &EngineContext,
        node: &Literal,
    ) -> Result<Value, RuntimeError> {
        Ok(match node {
            Literal::Boolean(value) => Value::Bool(*value),
            Literal::Int32(value) => Value::Number(f64::from(*value)),
            Literal::Hash(hash) => {
                let Some(resource) = self.resources.get(hash) else {
                    return Err(RuntimeError::UnknownHash(*hash));
                };

                match resource {
                    // Don't invoke the function, just return a reference.
                    Resource::FunctionDefinition { .. } => Value::FunctionRef(*hash),

                    // Treat it like a variable. Resolve the value.
                    Resource::Const(literal) => self.eval_literal_expr(ctx, literal)?,
                }
            }
            Literal::Identifier(id) => {
                let Some(value) = ctx.stack.context.get(id) else {
                    return Err(RuntimeError::UninitializedValue(*id));
                };

                // TODO: Avoid cloning values every time they are accessed.
                value.clone()
            }
        })
    }

    pub(self) fn eval_expr(
        &self,
        ctx: &EngineContext,
        node: &Expression,
    ) -> Result<Value, RuntimeError> {
        match node {
            // `#abc()`
            Expression::FunctionCall {
                callee,
                arguments: _,
            } => {
                let Expression::Literal(Literal::Hash(hash)) = **callee else {
                    return Err(RuntimeError::UnsupportedCallTarget(
                        "Not a hash literal.".to_string(),
                    ));
                };

                self.eval_function_call(ctx, &hash)
            }

            // `123`
            Expression::Literal(literal) => self.eval_literal_expr(ctx, literal),
        }
    }

    pub(self) fn eval_function_call(
        &self,
        ctx: &EngineContext,
        hash: &blake3::Hash,
    ) -> Result<Value, RuntimeError> {
        let resource = self
            .resources
            .get(hash)
            .ok_or(RuntimeError::UnknownHash(*hash))?;

        match resource {
            // TODO: Support parameters.
            Resource::FunctionDefinition { body, .. } => self.eval_expr(ctx, body),
            Resource::Const(_) => Err(RuntimeError::UnsupportedCallValue(*hash)),
        }
    }
}

#[derive(Default)]
pub(crate) struct EngineContext {
    /// The current stack frame.
    stack: StackFrame,
}

#[derive(Default)]
pub(crate) struct StackFrame {
    /// The parent stack frame, or none if this is the program's entry point.
    _parent: Option<Box<StackFrame>>,

    /// Local variables in this stack frame.
    context: HashMap<u16, Value>,
}

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

    #[error("Variable not found: {0}")]
    UninitializedValue(u16),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_literal_boolean() {
        let result = Interpreter::default().eval(&Expression::Literal(Literal::Boolean(true)));

        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn test_evaluate_literal_i32() {
        let result = Interpreter::default().eval(&Expression::Literal(Literal::Int32(42)));

        assert_eq!(result, Ok(Value::Number(42.0)));
    }

    #[test]
    fn test_evaluate_unknown_reference_fails() {
        let hash = blake3::hash(b"no-such-id");
        let result = Interpreter::default().eval(&Expression::Literal(Literal::Hash(hash)));

        assert_eq!(result, Err(RuntimeError::UnknownHash(hash)));
    }

    #[test]
    fn test_resolve_constant_value_from_hash() {
        let value = Resource::Const(Literal::Int32(1234));
        let hash = value.hash();

        let interpreter = Interpreter::with_resources(vec![value]);
        let result = interpreter.eval(&Expression::Literal(Literal::Hash(hash)));

        assert_eq!(result, Ok(Value::Number(1234.0)));
    }

    #[test]
    fn test_recursive_resolve_constant_value_from_hash() {
        let value = Resource::Const(Literal::Int32(1234));
        let hash = value.hash();

        let reference = Resource::Const(Literal::Hash(hash));
        let reference_hash = reference.hash();

        let interpreter = Interpreter::with_resources(vec![value, reference]);
        let result = interpreter.eval(&Expression::Literal(Literal::Hash(reference_hash)));

        assert_eq!(result, Ok(Value::Number(1234.0)));
    }

    #[test]
    fn test_resolve_function_ref_from_hash() {
        let value = Resource::FunctionDefinition {
            body: Box::new(Expression::Literal(Literal::Boolean(true))),
        };

        let hash = value.hash();
        let interpreter = Interpreter::with_resources(vec![value]);
        let result = interpreter.eval(&Expression::Literal(Literal::Hash(hash)));

        // Function is not called. It only returns a reference.
        assert_eq!(result, Ok(Value::FunctionRef(hash)));
    }

    #[test]
    fn test_eval_function_call() {
        let func = Resource::FunctionDefinition {
            body: Box::new(Expression::Literal(Literal::Boolean(false))),
        };

        let main = func.hash();
        let result = Interpreter::with_resources(vec![func]).call(&main);

        assert_eq!(result, Ok(Value::Bool(false)));
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

        let main = fn2.hash();
        let result = Interpreter::with_resources(vec![fn1, fn2]).call(&main);
        assert_eq!(result, Ok(Value::Bool(true)));
    }
}
