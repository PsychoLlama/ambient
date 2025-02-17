use crate::syntax::{Literal, Statement};

/// Resources that can be content addressed by hash. Outputs must be deterministic and globally
/// unique.
pub trait ContentHash {
    /// Add content to the running hash. Includes all descendents.
    fn update(&self, hash: &mut blake3::Hasher);

    /// Create a final hash of the content and all descendent nodes.
    fn hash(&self) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        self.update(&mut hash);

        hash.finalize()
    }
}

/// Identifier pool used to distinguish values of different types which coincidentally share the
/// same value. For example, `1u8` and `true` would hash to the same value without a type
/// specifier.
///
/// Resist the urge to manicure the list. These values must be stable over time.
#[repr(u16)]
#[derive(Clone)]
enum SyntaxType {
    // Literals
    LiteralInt32 = 0,
    LiteralBoolean = 1,
    LiteralHash = 2,

    // Statements
    StatementConst = 100,
}

impl ContentHash for SyntaxType {
    fn update(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&(self.clone() as u16).to_le_bytes());
    }
}

impl ContentHash for Literal {
    fn update(&self, hash: &mut blake3::Hasher) {
        match self {
            Literal::Int32(value) => {
                SyntaxType::LiteralInt32.update(hash);
                hash.update(&value.to_le_bytes());
            }
            Literal::Boolean(value) => {
                SyntaxType::LiteralBoolean.update(hash);
                hash.update(&[*value as u8]);
            }
            Literal::Hash(value) => {
                SyntaxType::LiteralHash.update(hash);
                hash.update(value.as_bytes());
            }
        };
    }
}

impl ContentHash for Statement {
    fn update(&self, hash: &mut blake3::Hasher) {
        match self {
            Statement::Const(value) => {
                SyntaxType::StatementConst.update(hash);
                value.update(hash);
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_type_equality() {
        let hash = blake3::hash(b"id");

        assert_eq!(Literal::Int32(1).hash(), Literal::Int32(1).hash());
        assert_eq!(Literal::Boolean(true).hash(), Literal::Boolean(true).hash());
        assert_eq!(Literal::Hash(hash).hash(), Literal::Hash(hash).hash());
    }

    #[test]
    fn test_primitive_value_inequality() {
        let hash1 = blake3::hash(b"id-1");
        let hash2 = blake3::hash(b"id-2");

        assert_ne!(Literal::Int32(1).hash(), Literal::Int32(2).hash());
        assert_ne!(
            Literal::Boolean(true).hash(),
            Literal::Boolean(false).hash()
        );
        assert_ne!(Literal::Hash(hash1).hash(), Literal::Hash(hash2).hash());
    }

    #[test]
    fn test_type_inequality() {
        let hash = blake3::hash(b"id");

        assert_ne!(Literal::Int32(1).hash(), Literal::Boolean(true).hash());
        assert_ne!(Literal::Int32(1).hash(), Literal::Hash(hash).hash());
        assert_ne!(Literal::Boolean(true).hash(), Literal::Hash(hash).hash());
    }

    #[test]
    fn test_const_equality() {
        let hash = blake3::hash(b"id");

        assert_eq!(
            Statement::Const(Literal::Int32(1)).hash(),
            Statement::Const(Literal::Int32(1)).hash()
        );
        assert_eq!(
            Statement::Const(Literal::Boolean(true)).hash(),
            Statement::Const(Literal::Boolean(true)).hash()
        );
        assert_eq!(
            Statement::Const(Literal::Hash(hash)).hash(),
            Statement::Const(Literal::Hash(hash)).hash()
        );
    }

    #[test]
    fn test_const_and_value_inequality() {
        let hash = blake3::hash(b"id-1");

        assert_ne!(
            Statement::Const(Literal::Int32(1)).hash(),
            Literal::Int32(1).hash()
        );
        assert_ne!(
            Statement::Const(Literal::Boolean(true)).hash(),
            Literal::Boolean(true).hash()
        );
        assert_ne!(
            Statement::Const(Literal::Hash(hash)).hash(),
            Literal::Hash(hash).hash()
        );
    }
}
