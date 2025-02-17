/// Syntax for defining literal values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    /// Signed integer literal.
    Int32(i32),

    /// Boolean literal.
    Boolean(bool),

    /// A content-addressed resource ID. (Usually a function.)
    Hash(blake3::Hash),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expression {
    Literal(Literal),

    /// May call a hash, inline func, identifier, property, etc.
    FunctionCall {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
}

/// Shared resources, i.e. values that are indexed by content hash, stored, and replicated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    /// Immutable value known at compile time.
    Const(Literal),

    /// Static function definition.
    FunctionDefinition {
        // TODO: Support parameters.
        body: Box<Expression>,
    },
}
