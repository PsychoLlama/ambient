//! Symbol database implementation using `SQLite`.
//!
//! The `SymbolDb` provides fast symbol lookup for LSP features and
//! supports incremental compilation through hash-based invalidation.

#![allow(
    clippy::missing_errors_doc,
    clippy::cast_possible_wrap,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines
)]

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::ast::Span;
use crate::types::Type;

use super::extract::{AbilityMethodInfo, DependencyInfo, EnumVariantInfo, SymbolInfo, SymbolKind};
use super::schema::{CREATE_TABLES, SCHEMA_VERSION};
use super::serialize::{deserialize_type, serialize_type};

/// Errors that can occur when working with the symbol database.
#[derive(Debug, thiserror::Error)]
pub enum SymbolDbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Schema version mismatch: expected {expected}, found {found}")]
    SchemaVersionMismatch { expected: i64, found: i64 },
    #[error("Deserialization error: {0}")]
    Deserialize(#[from] super::serialize::DeserializeError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A record representing a module in the database.
#[derive(Debug, Clone)]
pub struct ModuleRecord {
    pub id: i64,
    pub path: String,
    pub file_path: String,
    pub source_hash: blake3::Hash,
    pub export_hash: blake3::Hash,
    pub doc: Option<String>,
    pub updated_at: i64,
}

/// A record representing a symbol in the database.
#[derive(Debug, Clone)]
pub struct SymbolRecord {
    pub id: i64,
    pub module_id: i64,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub is_public: bool,
    pub type_signature: Type,
    pub span: Span,
    pub doc: Option<String>,
}

/// A symbol with its file path for workspace searches.
#[derive(Debug, Clone)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub module_path: String,
    pub span: Span,
    pub doc: Option<String>,
}

/// A definition location for go-to-definition.
#[derive(Debug, Clone)]
pub struct DefinitionLocation {
    pub file_path: String,
    pub span: Span,
}

/// Information needed to insert/update a module.
#[derive(Debug)]
pub struct ModuleInfo {
    pub path: String,
    pub file_path: String,
    pub source_hash: blake3::Hash,
    pub export_hash: blake3::Hash,
    pub doc: Option<String>,
    pub symbols: Vec<SymbolInfo>,
    pub dependencies: Vec<DependencyInfo>,
}

/// The symbol database.
pub struct SymbolDb {
    conn: Connection,
}

impl SymbolDb {
    /// Open a symbol database at the given path.
    ///
    /// Creates the database and tables if they don't exist.
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or initialized.
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
    ///
    /// # Errors
    /// Returns an error if the database cannot be created.
    pub fn open_in_memory() -> Result<Self, SymbolDbError> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Initialize the database schema.
    fn initialize(&mut self) -> Result<(), SymbolDbError> {
        // Enable foreign keys
        self.conn.execute("PRAGMA foreign_keys = ON", [])?;

        // First, ensure the schema_version table exists
        self.conn.execute_batch(CREATE_TABLES)?;

        // Check current schema version
        let version: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;

        match version {
            None => {
                // Fresh database - insert version
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) if v == SCHEMA_VERSION => {
                // Schema is current, nothing to do
            }
            Some(v) => {
                // Schema mismatch - for now, error out
                // Future: implement migrations
                return Err(SymbolDbError::SchemaVersionMismatch {
                    expected: SCHEMA_VERSION,
                    found: v,
                });
            }
        }

        Ok(())
    }

    /// Check if a module is up-to-date based on source hash.
    ///
    /// Returns true if the module exists in the database with the same source hash.
    pub fn is_module_up_to_date(
        &self,
        path: &str,
        source_hash: blake3::Hash,
    ) -> Result<bool, SymbolDbError> {
        let stored_hash: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT source_hash FROM modules WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()?;

        Ok(stored_hash.is_some_and(|h| hash_from_bytes(&h) == Some(source_hash)))
    }

    /// Get a module record by path.
    pub fn get_module(&self, path: &str) -> Result<Option<ModuleRecord>, SymbolDbError> {
        let result = self
            .conn
            .query_row(
                "SELECT id, path, file_path, source_hash, export_hash, doc, updated_at
                 FROM modules WHERE path = ?1",
                params![path],
                |row| {
                    let source_hash_bytes: Vec<u8> = row.get(3)?;
                    let export_hash_bytes: Vec<u8> = row.get(4)?;
                    let source_hash =
                        hash_from_bytes(&source_hash_bytes).unwrap_or_else(|| blake3::hash(b""));
                    let export_hash =
                        hash_from_bytes(&export_hash_bytes).unwrap_or_else(|| blake3::hash(b""));
                    Ok(ModuleRecord {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        file_path: row.get(2)?,
                        source_hash,
                        export_hash,
                        doc: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?;

        Ok(result)
    }

    /// Insert or update a module and its symbols.
    ///
    /// Returns true if the export hash changed (dependents may need recompilation).
    pub fn upsert_module(&mut self, info: &ModuleInfo) -> Result<bool, SymbolDbError> {
        let tx = self.conn.transaction()?;
        let export_hash_changed;

        {
            // Check if module exists and if export hash changed
            let existing: Option<(i64, Vec<u8>)> = tx
                .query_row(
                    "SELECT id, export_hash FROM modules WHERE path = ?1",
                    params![&info.path],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            let now = current_timestamp();

            if let Some((module_id, old_export_hash)) = existing {
                let old_hash = hash_from_bytes(&old_export_hash);
                export_hash_changed = old_hash != Some(info.export_hash);

                // Update existing module
                tx.execute(
                    "UPDATE modules SET file_path = ?1, source_hash = ?2, export_hash = ?3,
                     doc = ?4, updated_at = ?5 WHERE id = ?6",
                    params![
                        &info.file_path,
                        info.source_hash.as_bytes().as_slice(),
                        info.export_hash.as_bytes().as_slice(),
                        &info.doc,
                        now,
                        module_id
                    ],
                )?;

                // Delete old symbols (cascade will handle enum_variants and ability_methods)
                tx.execute(
                    "DELETE FROM symbols WHERE module_id = ?1",
                    params![module_id],
                )?;

                // Delete old dependencies
                tx.execute(
                    "DELETE FROM dependencies WHERE module_id = ?1",
                    params![module_id],
                )?;

                // Insert new symbols
                insert_symbols(&tx, module_id, &info.symbols)?;

                // Insert new dependencies
                insert_dependencies(&tx, module_id, &info.dependencies)?;
            } else {
                export_hash_changed = true; // New module = changed

                // Insert new module
                tx.execute(
                    "INSERT INTO modules (path, file_path, source_hash, export_hash, doc, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        &info.path,
                        &info.file_path,
                        info.source_hash.as_bytes().as_slice(),
                        info.export_hash.as_bytes().as_slice(),
                        &info.doc,
                        now
                    ],
                )?;

                let module_id = tx.last_insert_rowid();

                // Insert symbols
                insert_symbols(&tx, module_id, &info.symbols)?;

                // Insert dependencies
                insert_dependencies(&tx, module_id, &info.dependencies)?;
            }
        }

        tx.commit()?;
        Ok(export_hash_changed)
    }

    /// Remove a module and its symbols.
    pub fn remove_module(&mut self, path: &str) -> Result<(), SymbolDbError> {
        self.conn
            .execute("DELETE FROM modules WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// Look up a symbol by fully qualified name.
    pub fn lookup_symbol(
        &self,
        qualified_name: &str,
    ) -> Result<Option<SymbolRecord>, SymbolDbError> {
        let result = self
            .conn
            .query_row(
                "SELECT id, module_id, name, qualified_name, kind, is_public,
                        type_signature, span_start, span_end, doc
                 FROM symbols WHERE qualified_name = ?1",
                params![qualified_name],
                |row| {
                    let type_sig: String = row.get(6)?;
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, bool>(5)?,
                        type_sig,
                        row.get::<_, u32>(7)?,
                        row.get::<_, u32>(8)?,
                        row.get::<_, Option<String>>(9)?,
                    ))
                },
            )
            .optional()?;

        match result {
            Some((
                id,
                module_id,
                name,
                qualified_name,
                kind,
                is_public,
                type_sig,
                start,
                end,
                doc,
            )) => {
                let type_signature = deserialize_type(&type_sig)?;
                Ok(Some(SymbolRecord {
                    id,
                    module_id,
                    name,
                    qualified_name,
                    kind,
                    is_public,
                    type_signature,
                    span: Span::new(start, end),
                    doc,
                }))
            }
            None => Ok(None),
        }
    }

    /// Look up a symbol definition by qualified name, returning file path and span.
    ///
    /// This is optimized for go-to-definition operations.
    pub fn lookup_definition(
        &self,
        qualified_name: &str,
    ) -> Result<Option<DefinitionLocation>, SymbolDbError> {
        let result = self
            .conn
            .query_row(
                "SELECT m.file_path, s.span_start, s.span_end
                 FROM symbols s
                 JOIN modules m ON s.module_id = m.id
                 WHERE s.qualified_name = ?1",
                params![qualified_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .optional()?;

        match result {
            Some((file_path, start, end)) => Ok(Some(DefinitionLocation {
                file_path,
                span: Span::new(start, end),
            })),
            None => Ok(None),
        }
    }

    /// Get all symbols in a module.
    pub fn get_module_symbols(
        &self,
        module_path: &str,
    ) -> Result<Vec<SymbolRecord>, SymbolDbError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.module_id, s.name, s.qualified_name, s.kind, s.is_public,
                    s.type_signature, s.span_start, s.span_end, s.doc
             FROM symbols s
             JOIN modules m ON s.module_id = m.id
             WHERE m.path = ?1
             ORDER BY s.name",
        )?;

        let rows = stmt.query_map(params![module_path], |row| {
            let type_sig: String = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
                type_sig,
                row.get::<_, u32>(7)?,
                row.get::<_, u32>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;

        let mut symbols = Vec::new();
        for row in rows {
            let (id, module_id, name, qualified_name, kind, is_public, type_sig, start, end, doc) =
                row?;
            let type_signature = deserialize_type(&type_sig)?;
            symbols.push(SymbolRecord {
                id,
                module_id,
                name,
                qualified_name,
                kind,
                is_public,
                type_signature,
                span: Span::new(start, end),
                doc,
            });
        }

        Ok(symbols)
    }

    /// Search for symbols matching a query.
    pub fn search_symbols(&self, query: &str) -> Result<Vec<SymbolRecord>, SymbolDbError> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, module_id, name, qualified_name, kind, is_public,
                    type_signature, span_start, span_end, doc
             FROM symbols
             WHERE name LIKE ?1 OR qualified_name LIKE ?1
             ORDER BY name
             LIMIT 100",
        )?;

        let rows = stmt.query_map(params![pattern], |row| {
            let type_sig: String = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
                type_sig,
                row.get::<_, u32>(7)?,
                row.get::<_, u32>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })?;

        let mut symbols = Vec::new();
        for row in rows {
            let (id, module_id, name, qualified_name, kind, is_public, type_sig, start, end, doc) =
                row?;
            let type_signature = deserialize_type(&type_sig)?;
            symbols.push(SymbolRecord {
                id,
                module_id,
                name,
                qualified_name,
                kind,
                is_public,
                type_signature,
                span: Span::new(start, end),
                doc,
            });
        }

        Ok(symbols)
    }

    /// Search for symbols with file path information for workspace symbol search.
    pub fn search_workspace_symbols(
        &self,
        query: &str,
    ) -> Result<Vec<WorkspaceSymbol>, SymbolDbError> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT s.name, s.qualified_name, s.kind, m.file_path, m.path,
                    s.span_start, s.span_end, s.doc
             FROM symbols s
             JOIN modules m ON s.module_id = m.id
             WHERE s.name LIKE ?1 OR s.qualified_name LIKE ?1
             ORDER BY s.name
             LIMIT 100",
        )?;

        let rows = stmt.query_map(params![pattern], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;

        let mut symbols = Vec::new();
        for row in rows {
            let (name, qualified_name, kind, file_path, module_path, start, end, doc) = row?;
            symbols.push(WorkspaceSymbol {
                name,
                qualified_name,
                kind,
                file_path,
                module_path,
                span: Span::new(start, end),
                doc,
            });
        }

        Ok(symbols)
    }

    /// Get all modules that depend on a given module.
    pub fn get_dependents(&self, module_path: &str) -> Result<Vec<String>, SymbolDbError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT m.path
             FROM modules m
             JOIN dependencies d ON d.module_id = m.id
             WHERE d.depends_on_path LIKE ?1 OR d.depends_on_path = ?1",
        )?;

        // Match both exact path and paths that start with the module
        let pattern = format!("{module_path}.%");
        let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;

        let mut dependents = Vec::new();
        for row in rows {
            dependents.push(row?);
        }

        // Also check exact matches
        let mut exact_stmt = self.conn.prepare(
            "SELECT DISTINCT m.path
             FROM modules m
             JOIN dependencies d ON d.module_id = m.id
             WHERE d.depends_on_path = ?1",
        )?;
        let exact_rows =
            exact_stmt.query_map(params![module_path], |row| row.get::<_, String>(0))?;
        for row in exact_rows {
            let path = row?;
            if !dependents.contains(&path) {
                dependents.push(path);
            }
        }

        Ok(dependents)
    }

    /// Invalidate a module, returning all dependent module paths that may need recompilation.
    pub fn invalidate_module(&mut self, path: &str) -> Result<Vec<String>, SymbolDbError> {
        let dependents = self.get_dependents(path)?;
        self.remove_module(path)?;
        Ok(dependents)
    }
}

/// Insert symbols into the database.
fn insert_symbols(
    conn: &Connection,
    module_id: i64,
    symbols: &[SymbolInfo],
) -> Result<(), SymbolDbError> {
    for symbol in symbols {
        let kind_str = match &symbol.kind {
            SymbolKind::Function { .. } => "function",
            SymbolKind::Const => "const",
            SymbolKind::TypeAlias => "type_alias",
            SymbolKind::Enum { .. } => "enum",
            SymbolKind::Ability { .. } => "ability",
        };

        let type_sig = serialize_type(&symbol.type_signature);

        conn.execute(
            "INSERT INTO symbols (module_id, name, qualified_name, kind, is_public,
                                  type_signature, span_start, span_end, doc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                module_id,
                symbol.name.as_ref(),
                &symbol.qualified_name,
                kind_str,
                symbol.is_public,
                type_sig,
                symbol.span_start,
                symbol.span_end,
                symbol.doc.as_ref().map(|s| s.as_ref())
            ],
        )?;

        let symbol_id = conn.last_insert_rowid();

        // Insert enum variants
        if let SymbolKind::Enum { variants } = &symbol.kind {
            insert_enum_variants(conn, symbol_id, variants)?;
        }

        // Insert ability methods
        if let SymbolKind::Ability { methods } = &symbol.kind {
            insert_ability_methods(conn, symbol_id, methods)?;
        }
    }

    Ok(())
}

/// Insert enum variants into the database.
fn insert_enum_variants(
    conn: &Connection,
    enum_id: i64,
    variants: &[EnumVariantInfo],
) -> Result<(), SymbolDbError> {
    for variant in variants {
        let payload_type = variant.payload_type.as_ref().map(serialize_type);
        conn.execute(
            "INSERT INTO enum_variants (enum_id, name, payload_type, span_start, span_end)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                enum_id,
                variant.name.as_ref(),
                payload_type,
                variant.span.start,
                variant.span.end
            ],
        )?;
    }
    Ok(())
}

/// Insert ability methods into the database.
fn insert_ability_methods(
    conn: &Connection,
    ability_id: i64,
    methods: &[AbilityMethodInfo],
) -> Result<(), SymbolDbError> {
    for method in methods {
        let params_json =
            serde_json::to_string(&method.params.iter().map(serialize_type).collect::<Vec<_>>())
                .unwrap_or_else(|_| "[]".to_string());
        let return_type = serialize_type(&method.return_type);

        conn.execute(
            "INSERT INTO ability_methods (ability_id, name, params, return_type, span_start, span_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ability_id,
                method.name.as_ref(),
                params_json,
                return_type,
                method.span.start,
                method.span.end
            ],
        )?;
    }
    Ok(())
}

/// Insert dependencies into the database.
fn insert_dependencies(
    conn: &Connection,
    module_id: i64,
    dependencies: &[DependencyInfo],
) -> Result<(), SymbolDbError> {
    for dep in dependencies {
        let items_json = dep
            .imported_items
            .as_ref()
            .map(|items| serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string()));

        conn.execute(
            "INSERT INTO dependencies (module_id, depends_on_path, import_kind, imported_items)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                module_id,
                &dep.depends_on_path,
                dep.import_kind.as_str(),
                items_json
            ],
        )?;
    }
    Ok(())
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

/// Get current Unix timestamp.
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Type;
    use std::sync::Arc;

    fn make_test_symbol(name: &str, qualified_name: &str, is_public: bool) -> SymbolInfo {
        SymbolInfo {
            name: Arc::from(name),
            qualified_name: qualified_name.to_string(),
            kind: SymbolKind::Function {
                abilities: crate::types::AbilitySet::Empty,
            },
            is_public,
            type_signature: Type::function(vec![Type::Number], Type::Number),
            span_start: 0,
            span_end: 10,
            doc: None,
        }
    }

    fn make_test_module_info(path: &str) -> ModuleInfo {
        ModuleInfo {
            path: path.to_string(),
            file_path: format!("src/{path}.ab"),
            source_hash: blake3::hash(b"source"),
            export_hash: blake3::hash(b"export"),
            doc: None,
            symbols: vec![
                make_test_symbol("foo", &format!("{path}.foo"), true),
                make_test_symbol("bar", &format!("{path}.bar"), false),
            ],
            dependencies: vec![],
        }
    }

    #[test]
    fn test_open_in_memory() {
        let db = SymbolDb::open_in_memory().expect("open failed");
        drop(db);
    }

    #[test]
    fn test_upsert_and_lookup() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let info = make_test_module_info("utils.math");

        db.upsert_module(&info).expect("upsert failed");

        // Lookup by qualified name
        let symbol = db
            .lookup_symbol("utils.math.foo")
            .expect("lookup failed")
            .expect("symbol not found");
        assert_eq!(symbol.name, "foo");
        assert!(symbol.is_public);

        // Private symbol should also be found
        let private = db
            .lookup_symbol("utils.math.bar")
            .expect("lookup failed")
            .expect("symbol not found");
        assert_eq!(private.name, "bar");
        assert!(!private.is_public);
    }

    #[test]
    fn test_is_module_up_to_date() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let info = make_test_module_info("test.module");

        // Before insert, not up to date
        assert!(!db
            .is_module_up_to_date("test.module", info.source_hash)
            .expect("check failed"));

        db.upsert_module(&info).expect("upsert failed");

        // After insert with same hash, up to date
        assert!(db
            .is_module_up_to_date("test.module", info.source_hash)
            .expect("check failed"));

        // Different hash, not up to date
        let different_hash = blake3::hash(b"different");
        assert!(!db
            .is_module_up_to_date("test.module", different_hash)
            .expect("check failed"));
    }

    #[test]
    fn test_get_module_symbols() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let info = make_test_module_info("test.module");

        db.upsert_module(&info).expect("upsert failed");

        let symbols = db.get_module_symbols("test.module").expect("get failed");
        assert_eq!(symbols.len(), 2);
    }

    #[test]
    fn test_search_symbols() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let info = make_test_module_info("test.module");

        db.upsert_module(&info).expect("upsert failed");

        let results = db.search_symbols("foo").expect("search failed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "foo");

        let results = db.search_symbols("test").expect("search failed");
        assert_eq!(results.len(), 2); // Both match qualified name
    }

    #[test]
    fn test_remove_module() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let info = make_test_module_info("test.module");

        db.upsert_module(&info).expect("upsert failed");
        assert!(db.get_module("test.module").expect("get failed").is_some());

        db.remove_module("test.module").expect("remove failed");
        assert!(db.get_module("test.module").expect("get failed").is_none());
    }

    #[test]
    fn test_export_hash_change_detection() {
        let mut db = SymbolDb::open_in_memory().expect("open failed");
        let mut info = make_test_module_info("test.module");

        // First insert - always changed
        let changed = db.upsert_module(&info).expect("upsert failed");
        assert!(changed);

        // Same info - not changed (same export hash)
        let changed = db.upsert_module(&info).expect("upsert failed");
        assert!(!changed);

        // Different export hash - changed
        info.export_hash = blake3::hash(b"different");
        let changed = db.upsert_module(&info).expect("upsert failed");
        assert!(changed);
    }
}
