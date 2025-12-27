//! Symbol database implementation using `SQLite`.
//!
//! The `SymbolDb` provides bidirectional hash ↔ symbol path lookups,
//! hash-level dependency tracking for find-references, and type registry
//! for hover/completions.

#![allow(
    clippy::missing_errors_doc,
    clippy::cast_possible_wrap,
    clippy::redundant_closure_for_method_calls
)]

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::types::Type;

use super::schema::CREATE_SCHEMA;
use super::serialize::{deserialize_type, serialize_type};

/// Errors that can occur when working with the symbol database.
#[derive(Debug, thiserror::Error)]
pub enum SymbolDbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Deserialization error: {0}")]
    Deserialize(#[from] super::serialize::DeserializeError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Hash not found in database")]
    HashNotFound,
}

/// Kind of symbol stored in `symbol_paths` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Const,
    Enum,
    Ability,
}

impl SymbolKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Const => "const",
            Self::Enum => "enum",
            Self::Ability => "ability",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "function" => Some(Self::Function),
            "const" => Some(Self::Const),
            "enum" => Some(Self::Enum),
            "ability" => Some(Self::Ability),
            _ => None,
        }
    }
}

/// Kind of type stored in types table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Named,
    Enum,
    Alias,
    AnonymousRecord,
    AnonymousTuple,
}

impl TypeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Named => "named",
            Self::Enum => "enum",
            Self::Alias => "alias",
            Self::AnonymousRecord => "anonymous_record",
            Self::AnonymousTuple => "anonymous_tuple",
        }
    }

    #[allow(dead_code)]
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "named" => Some(Self::Named),
            "enum" => Some(Self::Enum),
            "alias" => Some(Self::Alias),
            "anonymous_record" => Some(Self::AnonymousRecord),
            "anonymous_tuple" => Some(Self::AnonymousTuple),
            _ => None,
        }
    }
}

/// Kind of dependency between hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Call,
    TypeRef,
    Capture,
}

impl DependencyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TypeRef => "type_ref",
            Self::Capture => "capture",
        }
    }
}

/// A symbol path entry from the database.
#[derive(Debug, Clone)]
pub struct SymbolPathEntry {
    pub path: String,
    pub kind: SymbolKind,
    pub module_path: String,
    pub hash: blake3::Hash,
}

/// A type entry from the database.
#[derive(Debug, Clone)]
pub struct TypeEntry {
    pub path: String,
    pub kind: TypeKind,
    pub module_path: String,
    pub type_hash: blake3::Hash,
    pub signature: Type,
    pub parent_symbol: Option<String>,
}

/// The symbol database.
pub struct SymbolDb {
    conn: Connection,
}

impl SymbolDb {
    /// Open a symbol database at the given path.
    ///
    /// Creates the database and tables if they don't exist.
    pub fn open(path: &Path) -> Result<Self, SymbolDbError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        let mut db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Open an in-memory symbol database for testing.
    pub fn open_in_memory() -> Result<Self, SymbolDbError> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Initialize the database schema.
    fn initialize(&mut self) -> Result<(), SymbolDbError> {
        self.conn.execute("PRAGMA foreign_keys = ON", [])?;
        self.conn.execute_batch(CREATE_SCHEMA)?;
        Ok(())
    }

    // ========================================================================
    // Hash Registry
    // ========================================================================

    /// Get or create a hash ID in the hashes table.
    fn get_or_create_hash_id(&self, hash: blake3::Hash) -> Result<i64, SymbolDbError> {
        let hash_bytes = hash.as_bytes().as_slice();

        // Try to find existing
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM hashes WHERE hash = ?1",
                params![hash_bytes],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            return Ok(id);
        }

        // Insert new
        self.conn
            .execute("INSERT INTO hashes (hash) VALUES (?1)", params![hash_bytes])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get hash ID if it exists.
    fn get_hash_id(&self, hash: blake3::Hash) -> Result<Option<i64>, SymbolDbError> {
        let hash_bytes = hash.as_bytes().as_slice();
        let result = self
            .conn
            .query_row(
                "SELECT id FROM hashes WHERE hash = ?1",
                params![hash_bytes],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    /// Get hash by ID.
    fn get_hash_by_id(&self, id: i64) -> Result<blake3::Hash, SymbolDbError> {
        let bytes: Vec<u8> = self.conn.query_row(
            "SELECT hash FROM hashes WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        hash_from_bytes(&bytes).ok_or(SymbolDbError::HashNotFound)
    }

    // ========================================================================
    // Symbol Path Lookups (Bidirectional)
    // ========================================================================

    /// Get the hash for a symbol path.
    pub fn get_hash(&self, symbol_path: &str) -> Result<Option<blake3::Hash>, SymbolDbError> {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT hash_id FROM symbol_paths WHERE path = ?1",
                params![symbol_path],
                |row| row.get(0),
            )
            .optional()?;

        match result {
            Some(hash_id) => Ok(Some(self.get_hash_by_id(hash_id)?)),
            None => Ok(None),
        }
    }

    /// Get all symbol paths that map to a given hash.
    pub fn get_symbol_paths(&self, hash: blake3::Hash) -> Result<Vec<String>, SymbolDbError> {
        let Some(hash_id) = self.get_hash_id(hash)? else {
            return Ok(Vec::new());
        };

        let mut stmt = self
            .conn
            .prepare("SELECT path FROM symbol_paths WHERE hash_id = ?1")?;
        let rows = stmt.query_map(params![hash_id], |row| row.get::<_, String>(0))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(row?);
        }
        Ok(paths)
    }

    /// Register a symbol with its hash.
    pub fn register_symbol(
        &mut self,
        path: &str,
        kind: SymbolKind,
        module_path: &str,
        hash: blake3::Hash,
    ) -> Result<(), SymbolDbError> {
        let hash_id = self.get_or_create_hash_id(hash)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO symbol_paths (path, kind, module_path, hash_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![path, kind.as_str(), module_path, hash_id],
        )?;
        Ok(())
    }

    /// Look up a symbol path entry.
    pub fn lookup_symbol_path(&self, path: &str) -> Result<Option<SymbolPathEntry>, SymbolDbError> {
        let result: Option<(String, String, i64)> = self
            .conn
            .query_row(
                "SELECT kind, module_path, hash_id FROM symbol_paths WHERE path = ?1",
                params![path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        match result {
            Some((kind_str, module_path, hash_id)) => {
                let kind = SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Function);
                let hash = self.get_hash_by_id(hash_id)?;
                Ok(Some(SymbolPathEntry {
                    path: path.to_string(),
                    kind,
                    module_path,
                    hash,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get the module path for a symbol path.
    pub fn get_module_path(&self, symbol_path: &str) -> Result<Option<String>, SymbolDbError> {
        let result = self
            .conn
            .query_row(
                "SELECT module_path FROM symbol_paths WHERE path = ?1",
                params![symbol_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(result)
    }

    /// Get all symbol paths in a module.
    pub fn get_module_symbols(
        &self,
        module_path: &str,
    ) -> Result<Vec<SymbolPathEntry>, SymbolDbError> {
        let mut stmt = self.conn.prepare(
            "SELECT sp.path, sp.kind, sp.module_path, h.hash
             FROM symbol_paths sp
             JOIN hashes h ON sp.hash_id = h.id
             WHERE sp.module_path = ?1
             ORDER BY sp.path",
        )?;
        let rows = stmt.query_map(params![module_path], |row| {
            let hash_bytes: [u8; 32] = row.get::<_, Vec<u8>>(3)?.try_into().map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Blob,
                    "Invalid hash length".into(),
                )
            })?;
            Ok(SymbolPathEntry {
                path: row.get(0)?,
                kind: SymbolKind::from_str(&row.get::<_, String>(1)?)
                    .unwrap_or(SymbolKind::Function),
                module_path: row.get(2)?,
                hash: blake3::Hash::from_bytes(hash_bytes),
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Search for symbol paths matching a query.
    pub fn search_symbols(&self, query: &str) -> Result<Vec<SymbolPathEntry>, SymbolDbError> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT path, kind, module_path, hash_id FROM symbol_paths
             WHERE path LIKE ?1
             ORDER BY path
             LIMIT 100",
        )?;

        let rows = stmt.query_map(params![pattern], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            let (path, kind_str, module_path, hash_id) = row?;
            let kind = SymbolKind::from_str(&kind_str).unwrap_or(SymbolKind::Function);
            let hash = self.get_hash_by_id(hash_id)?;
            entries.push(SymbolPathEntry {
                path,
                kind,
                module_path,
                hash,
            });
        }
        Ok(entries)
    }

    // ========================================================================
    // Lambda Parent Tracking
    // ========================================================================

    /// Register a lambda with its parent function's symbol path.
    pub fn register_lambda(
        &mut self,
        lambda_hash: blake3::Hash,
        parent_path: &str,
    ) -> Result<(), SymbolDbError> {
        let hash_id = self.get_or_create_hash_id(lambda_hash)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO lambda_parents (hash_id, parent_path)
             VALUES (?1, ?2)",
            params![hash_id, parent_path],
        )?;
        Ok(())
    }

    /// Get the parent symbol path for a lambda hash.
    pub fn get_lambda_parent(
        &self,
        lambda_hash: blake3::Hash,
    ) -> Result<Option<String>, SymbolDbError> {
        let Some(hash_id) = self.get_hash_id(lambda_hash)? else {
            return Ok(None);
        };

        let result = self
            .conn
            .query_row(
                "SELECT parent_path FROM lambda_parents WHERE hash_id = ?1",
                params![hash_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(result)
    }

    /// Get all lambda hashes for a parent function.
    pub fn get_lambdas_for_parent(
        &self,
        parent_path: &str,
    ) -> Result<Vec<blake3::Hash>, SymbolDbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT hash_id FROM lambda_parents WHERE parent_path = ?1")?;
        let rows = stmt.query_map(params![parent_path], |row| row.get::<_, i64>(0))?;

        let mut hashes = Vec::new();
        for row in rows {
            let hash_id = row?;
            hashes.push(self.get_hash_by_id(hash_id)?);
        }
        Ok(hashes)
    }

    // ========================================================================
    // Type Registry
    // ========================================================================

    /// Register a type.
    pub fn register_type(
        &mut self,
        path: &str,
        kind: TypeKind,
        module_path: &str,
        type_hash: blake3::Hash,
        signature: &Type,
        parent_symbol: Option<&str>,
    ) -> Result<(), SymbolDbError> {
        let signature_json = serialize_type(signature);
        let type_hash_bytes = type_hash.as_bytes().as_slice();

        self.conn.execute(
            "INSERT OR REPLACE INTO types (path, kind, module_path, type_hash, signature, parent_symbol)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                path,
                kind.as_str(),
                module_path,
                type_hash_bytes,
                signature_json,
                parent_symbol
            ],
        )?;
        Ok(())
    }

    /// Look up a type by path.
    #[allow(clippy::type_complexity)]
    pub fn get_type(&self, path: &str) -> Result<Option<TypeEntry>, SymbolDbError> {
        let result: Option<(String, String, Vec<u8>, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT kind, module_path, type_hash, signature, parent_symbol
                 FROM types WHERE path = ?1",
                params![path],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;

        match result {
            Some((kind_str, module_path, type_hash_bytes, signature_json, parent_symbol)) => {
                let kind = TypeKind::from_str(&kind_str).unwrap_or(TypeKind::Named);
                let type_hash =
                    hash_from_bytes(&type_hash_bytes).ok_or(SymbolDbError::HashNotFound)?;
                let signature = deserialize_type(&signature_json)?;
                Ok(Some(TypeEntry {
                    path: path.to_string(),
                    kind,
                    module_path,
                    type_hash,
                    signature,
                    parent_symbol,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get types by structural hash (may return multiple).
    pub fn get_types_by_hash(
        &self,
        type_hash: blake3::Hash,
    ) -> Result<Vec<TypeEntry>, SymbolDbError> {
        let type_hash_bytes = type_hash.as_bytes().as_slice();
        let mut stmt = self.conn.prepare(
            "SELECT path, kind, module_path, signature, parent_symbol
             FROM types WHERE type_hash = ?1",
        )?;

        let rows = stmt.query_map(params![type_hash_bytes], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            let (path, kind_str, module_path, signature_json, parent_symbol) = row?;
            let kind = TypeKind::from_str(&kind_str).unwrap_or(TypeKind::Named);
            let signature = deserialize_type(&signature_json)?;
            entries.push(TypeEntry {
                path,
                kind,
                module_path,
                type_hash,
                signature,
                parent_symbol,
            });
        }
        Ok(entries)
    }

    /// Get type signature for a symbol (for hover).
    pub fn get_type_signature(&self, path: &str) -> Result<Option<Type>, SymbolDbError> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT signature FROM types WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()?;

        match result {
            Some(json) => Ok(Some(deserialize_type(&json)?)),
            None => Ok(None),
        }
    }

    // ========================================================================
    // Hash-Level Dependencies (for Find References)
    // ========================================================================

    /// Record a dependency from one hash to another.
    pub fn add_dependency(
        &mut self,
        from: blake3::Hash,
        to: blake3::Hash,
        kind: DependencyKind,
    ) -> Result<(), SymbolDbError> {
        let from_id = self.get_or_create_hash_id(from)?;
        let to_id = self.get_or_create_hash_id(to)?;

        self.conn.execute(
            "INSERT OR IGNORE INTO hash_dependencies (dependent_hash_id, dependency_hash_id, kind)
             VALUES (?1, ?2, ?3)",
            params![from_id, to_id, kind.as_str()],
        )?;
        Ok(())
    }

    /// Get all hashes that depend on the given hash (reverse lookup for "find references").
    pub fn get_dependents(&self, hash: blake3::Hash) -> Result<Vec<blake3::Hash>, SymbolDbError> {
        let Some(hash_id) = self.get_hash_id(hash)? else {
            return Ok(Vec::new());
        };

        let mut stmt = self.conn.prepare(
            "SELECT dependent_hash_id FROM hash_dependencies WHERE dependency_hash_id = ?1",
        )?;
        let rows = stmt.query_map(params![hash_id], |row| row.get::<_, i64>(0))?;

        let mut hashes = Vec::new();
        for row in rows {
            let dep_id = row?;
            hashes.push(self.get_hash_by_id(dep_id)?);
        }
        Ok(hashes)
    }

    /// Get all hashes that the given hash depends on.
    pub fn get_dependencies(&self, hash: blake3::Hash) -> Result<Vec<blake3::Hash>, SymbolDbError> {
        let Some(hash_id) = self.get_hash_id(hash)? else {
            return Ok(Vec::new());
        };

        let mut stmt = self.conn.prepare(
            "SELECT dependency_hash_id FROM hash_dependencies WHERE dependent_hash_id = ?1",
        )?;
        let rows = stmt.query_map(params![hash_id], |row| row.get::<_, i64>(0))?;

        let mut hashes = Vec::new();
        for row in rows {
            let dep_id = row?;
            hashes.push(self.get_hash_by_id(dep_id)?);
        }
        Ok(hashes)
    }

    // ========================================================================
    // Cleanup / Garbage Collection
    // ========================================================================

    /// Remove all symbol paths for a module.
    pub fn remove_module(&mut self, module_path: &str) -> Result<(), SymbolDbError> {
        // Remove symbol paths
        self.conn.execute(
            "DELETE FROM symbol_paths WHERE module_path = ?1",
            params![module_path],
        )?;

        // Remove types
        self.conn.execute(
            "DELETE FROM types WHERE module_path = ?1",
            params![module_path],
        )?;

        // Lambda parents for symbols in this module need cleanup too
        // They reference parent_path which includes module_path prefix
        self.conn.execute(
            "DELETE FROM lambda_parents WHERE parent_path LIKE ?1",
            params![format!("{module_path}.%")],
        )?;

        Ok(())
    }

    /// Garbage collect orphaned hashes (hashes with no references).
    /// Returns the number of hashes removed.
    pub fn gc_orphaned_hashes(&mut self) -> Result<usize, SymbolDbError> {
        // Delete hashes that are not referenced by symbol_paths or lambda_parents
        let deleted = self.conn.execute(
            "DELETE FROM hashes WHERE id NOT IN (
                SELECT hash_id FROM symbol_paths
                UNION
                SELECT hash_id FROM lambda_parents
            )",
            [],
        )?;
        Ok(deleted)
    }

    /// Full cleanup: remove module and run GC.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn cleanup_module(&mut self, module_path: &str) -> Result<CleanupStats, SymbolDbError> {
        let tx = self.conn.transaction()?;

        // Count before
        let symbols_before: i64 = tx.query_row(
            "SELECT COUNT(*) FROM symbol_paths WHERE module_path = ?1",
            params![module_path],
            |row| row.get(0),
        )?;
        let types_before: i64 = tx.query_row(
            "SELECT COUNT(*) FROM types WHERE module_path = ?1",
            params![module_path],
            |row| row.get(0),
        )?;

        // Remove module data
        tx.execute(
            "DELETE FROM symbol_paths WHERE module_path = ?1",
            params![module_path],
        )?;
        tx.execute(
            "DELETE FROM types WHERE module_path = ?1",
            params![module_path],
        )?;
        tx.execute(
            "DELETE FROM lambda_parents WHERE parent_path LIKE ?1",
            params![format!("{module_path}.%")],
        )?;

        // GC orphaned hashes
        let hashes_removed = tx.execute(
            "DELETE FROM hashes WHERE id NOT IN (
                SELECT hash_id FROM symbol_paths
                UNION
                SELECT hash_id FROM lambda_parents
            )",
            [],
        )?;

        tx.commit()?;

        Ok(CleanupStats {
            symbols_removed: symbols_before as usize,
            types_removed: types_before as usize,
            hashes_removed,
        })
    }
}

/// Statistics from cleanup operation.
#[derive(Debug, Default)]
pub struct CleanupStats {
    pub symbols_removed: usize,
    pub types_removed: usize,
    pub hashes_removed: usize,
}

/// Statistics from module population.
#[derive(Debug, Default)]
pub struct PopulateStats {
    /// Number of symbols registered.
    pub symbols_registered: usize,
    /// Number of lambdas registered.
    pub lambdas_registered: usize,
    /// Number of dependencies recorded.
    pub dependencies_recorded: usize,
}

impl SymbolDb {
    /// Populate the database from a compiled module.
    ///
    /// This registers all named symbols and their dependencies from the compiled module.
    /// The `module_path` should be the fully qualified module path (e.g., "mylib.utils").
    /// The `package_name` is the package name for constructing symbol paths.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let compiled = compile_module(&ast)?;
    /// db.populate_from_module(&compiled, "mylib", "mylib.utils", &function_map)?;
    /// ```
    /// Populate the database from a compiled module.
    ///
    /// Registers all named symbols, lambdas, and their dependencies.
    /// The `function_visibility` map is currently unused but reserved for
    /// future filtering of exported symbols.
    pub fn populate_from_module(
        &mut self,
        compiled: &crate::compiler::CompiledModule,
        package_name: &str,
        module_path: &str,
        _function_visibility: &std::collections::HashMap<std::sync::Arc<str>, bool>,
    ) -> Result<PopulateStats, SymbolDbError> {
        let mut stats = PopulateStats::default();

        // Register each named function
        for (name, hash) in &compiled.function_names {
            // Build the full symbol path: package.module.name
            let symbol_path = if module_path.is_empty() {
                format!("{package_name}.{name}")
            } else {
                format!("{package_name}.{module_path}.{name}")
            };

            self.register_symbol(&symbol_path, SymbolKind::Function, module_path, *hash)?;
            stats.symbols_registered += 1;

            // Record dependencies for this function
            if let Some(func) = compiled.functions.get(hash) {
                for dep_hash in &func.dependencies {
                    self.add_dependency(*hash, *dep_hash, DependencyKind::Call)?;
                    stats.dependencies_recorded += 1;
                }
            }
        }

        // Register lambdas with their parent functions
        for (lambda_hash, parent_name) in &compiled.lambda_parents {
            // Build the full parent symbol path: package.module.name
            let parent_path = if module_path.is_empty() {
                format!("{package_name}.{parent_name}")
            } else {
                format!("{package_name}.{module_path}.{parent_name}")
            };

            self.register_lambda(*lambda_hash, &parent_path)?;
            stats.lambdas_registered += 1;

            // Record dependencies for this lambda
            if let Some(func) = compiled.functions.get(lambda_hash) {
                for dep_hash in &func.dependencies {
                    self.add_dependency(*lambda_hash, *dep_hash, DependencyKind::Call)?;
                    stats.dependencies_recorded += 1;
                }
            }
        }

        Ok(stats)
    }
}

/// Convert bytes to a blake3 hash.
fn hash_from_bytes(bytes: &[u8]) -> Option<blake3::Hash> {
    if bytes.len() == 32 {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(bytes);
        Some(blake3::Hash::from_bytes(arr))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let db = SymbolDb::open_in_memory().expect("open failed");
        drop(db);
    }

    #[test]
    fn test_register_and_lookup_symbol() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let hash = blake3::hash(b"test function");

        db.register_symbol(
            "mylib.utils.format",
            SymbolKind::Function,
            "mylib.utils",
            hash,
        )
        .expect("register failed");

        // Forward lookup: path -> hash
        let found_hash = db.get_hash("mylib.utils.format").expect("lookup failed");
        assert_eq!(found_hash, Some(hash));

        // Reverse lookup: hash -> paths
        let paths = db.get_symbol_paths(hash).expect("reverse lookup failed");
        assert_eq!(paths, vec!["mylib.utils.format"]);

        // Full entry lookup
        let entry = db
            .lookup_symbol_path("mylib.utils.format")
            .expect("lookup failed")
            .expect("not found");
        assert_eq!(entry.kind, SymbolKind::Function);
        assert_eq!(entry.module_path, "mylib.utils");
        assert_eq!(entry.hash, hash);
    }

    #[test]
    fn test_multiple_paths_same_hash() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let hash = blake3::hash(b"identical implementation");

        db.register_symbol(
            "lib1.utils.format",
            SymbolKind::Function,
            "lib1.utils",
            hash,
        )
        .expect("register failed");
        db.register_symbol(
            "lib2.utils.format",
            SymbolKind::Function,
            "lib2.utils",
            hash,
        )
        .expect("register failed");

        let paths = db.get_symbol_paths(hash).expect("lookup failed");
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"lib1.utils.format".to_string()));
        assert!(paths.contains(&"lib2.utils.format".to_string()));
    }

    #[test]
    fn test_lambda_parent() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let lambda_hash = blake3::hash(b"lambda body");

        db.register_lambda(lambda_hash, "mylib.utils.process")
            .expect("register failed");

        let parent = db.get_lambda_parent(lambda_hash).expect("lookup failed");
        assert_eq!(parent, Some("mylib.utils.process".to_string()));

        let lambdas = db
            .get_lambdas_for_parent("mylib.utils.process")
            .expect("lookup failed");
        assert_eq!(lambdas, vec![lambda_hash]);
    }

    #[test]
    fn test_type_registry() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let type_hash = blake3::hash(b"type definition");
        let signature = Type::function(vec![Type::Number], Type::String);

        db.register_type(
            "mylib.models.User",
            TypeKind::Named,
            "mylib.models",
            type_hash,
            &signature,
            None,
        )
        .expect("register failed");

        let entry = db
            .get_type("mylib.models.User")
            .expect("lookup failed")
            .expect("not found");
        assert_eq!(entry.kind, TypeKind::Named);
        assert_eq!(entry.type_hash, type_hash);
        assert_eq!(entry.signature, signature);
    }

    #[test]
    fn test_dependencies() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let caller_hash = blake3::hash(b"caller");
        let callee_hash = blake3::hash(b"callee");

        db.add_dependency(caller_hash, callee_hash, DependencyKind::Call)
            .expect("add failed");

        // Forward: what does caller depend on?
        let deps = db.get_dependencies(caller_hash).expect("lookup failed");
        assert_eq!(deps, vec![callee_hash]);

        // Reverse: who calls callee?
        let refs = db.get_dependents(callee_hash).expect("lookup failed");
        assert_eq!(refs, vec![caller_hash]);
    }

    #[test]
    fn test_remove_module() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let hash = blake3::hash(b"test");

        db.register_symbol("mylib.utils.foo", SymbolKind::Function, "mylib.utils", hash)
            .expect("register failed");
        db.register_symbol("mylib.utils.bar", SymbolKind::Function, "mylib.utils", hash)
            .expect("register failed");

        assert!(db.get_hash("mylib.utils.foo").expect("lookup").is_some());

        db.remove_module("mylib.utils").expect("remove failed");

        assert!(db.get_hash("mylib.utils.foo").expect("lookup").is_none());
        assert!(db.get_hash("mylib.utils.bar").expect("lookup").is_none());
    }

    #[test]
    fn test_gc_orphaned_hashes() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let hash = blake3::hash(b"test");

        db.register_symbol("mylib.utils.foo", SymbolKind::Function, "mylib.utils", hash)
            .expect("register failed");

        // Remove symbol but hash remains
        db.remove_module("mylib.utils").expect("remove failed");

        // GC should clean up orphaned hash
        let removed = db.gc_orphaned_hashes().expect("gc failed");
        assert_eq!(removed, 1);
    }

    #[test]
    fn test_search_symbols() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let hash = blake3::hash(b"test");

        db.register_symbol(
            "mylib.utils.format",
            SymbolKind::Function,
            "mylib.utils",
            hash,
        )
        .expect("register failed");
        db.register_symbol(
            "mylib.utils.parse",
            SymbolKind::Function,
            "mylib.utils",
            hash,
        )
        .expect("register failed");

        let results = db.search_symbols("format").expect("search failed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "mylib.utils.format");

        let results = db.search_symbols("utils").expect("search failed");
        assert_eq!(results.len(), 2);
    }
}
