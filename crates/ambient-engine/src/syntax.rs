/// Syntax for defining literal values.
#[derive(Clone, Debug)]
pub enum Literal {
    /// Signed integer literal.
    Int32(i32),

    /// Boolean literal.
    Boolean(bool),

    /// A content-addressed resource ID. (Usually a function.)
    Hash(blake3::Hash),
}

#[derive(Clone, Debug)]
pub enum Expression {
    Literal(Literal),

    /// May call a hash, inline func, identifier, property, etc.
    FunctionCall {
        callee: Box<crate::value::Value>,
        arguments: Vec<Expression>,
    },

    /// Static function definition.
    FunctionDefinition {
        parameters: Vec<String>,
        body: Box<Expression>,
    },
}
