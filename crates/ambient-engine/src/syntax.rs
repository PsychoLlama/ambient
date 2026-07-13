/// Syntax for defining literal values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    /// Signed integer literal.
    Int32(i32),

    /// Boolean literal.
    Boolean(bool),

    /// A content-addressed resource ID. (Usually a function.)
    Hash(blake3::Hash),

    /// Variable identifier. Each new identifier increments the counter.
    Identifier(u16),
}

/// Expressions that return a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expression {
    Literal(Literal),

    /// May call a hash, inline func, identifier, property, etc.
    FunctionCall {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
}

/// Expressions that do not return a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Statement {
    /// Binds a value to a variable. IDs increment for each new variable in scope.
    Binding { id: u16, value: Expression },
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
