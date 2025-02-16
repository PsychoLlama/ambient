use crate::{syntax::Literal, value::Value};

fn eval_literal_expr(node: &Literal) -> Value {
    match node {
        Literal::Boolean(value) => Value::Boolean(*value),
        Literal::Int32(value) => Value::Int32(*value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_literal_boolean() {
        let node = Literal::Boolean(true);
        let result = eval_literal_expr(&node);
        assert_eq!(result, Value::Boolean(true));
    }

    #[test]
    fn test_evaluate_literal_i32() {
        let node = Literal::Int32(42);
        let result = eval_literal_expr(&node);
        assert_eq!(result, Value::Int32(42));
    }
}
