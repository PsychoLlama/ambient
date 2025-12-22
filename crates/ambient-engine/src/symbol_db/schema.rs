//! SQL schema definitions for the symbol database.

/// Current schema version for migration support.
pub const SCHEMA_VERSION: i64 = 1;

/// SQL to create all tables.
pub const CREATE_TABLES: &str = r#"
-- Schema version for migrations
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);

-- Module information
CREATE TABLE IF NOT EXISTS modules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Canonical module path (e.g., "utils.format")
    path TEXT UNIQUE NOT NULL,
    -- Source file path (relative to src dir)
    file_path TEXT NOT NULL,
    -- Hash of source file content (32 bytes)
    source_hash BLOB NOT NULL,
    -- Hash of public exports for dependency invalidation (32 bytes)
    export_hash BLOB NOT NULL,
    -- Module-level documentation
    doc TEXT,
    -- Last modified timestamp
    updated_at INTEGER NOT NULL
);

-- Symbols (functions, constants, types, enums, abilities)
CREATE TABLE IF NOT EXISTS symbols (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    module_id INTEGER NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    -- Symbol name (unqualified)
    name TEXT NOT NULL,
    -- Fully qualified name (e.g., "utils.format.helper")
    qualified_name TEXT NOT NULL UNIQUE,
    -- Symbol kind: 'function', 'const', 'type_alias', 'enum', 'ability'
    kind TEXT NOT NULL,
    -- Is this symbol public?
    is_public INTEGER NOT NULL DEFAULT 0,
    -- Serialized type signature (JSON)
    type_signature TEXT NOT NULL,
    -- Source span start offset
    span_start INTEGER NOT NULL,
    -- Source span end offset
    span_end INTEGER NOT NULL,
    -- Documentation from /// comments
    doc TEXT
);

-- Enum variants (child symbols of enums)
CREATE TABLE IF NOT EXISTS enum_variants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    enum_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- Payload type (JSON), NULL if no payload
    payload_type TEXT,
    span_start INTEGER NOT NULL,
    span_end INTEGER NOT NULL
);

-- Ability methods (child symbols of abilities)
CREATE TABLE IF NOT EXISTS ability_methods (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ability_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- Parameter types (JSON array)
    params TEXT NOT NULL,
    -- Return type (JSON)
    return_type TEXT NOT NULL,
    span_start INTEGER NOT NULL,
    span_end INTEGER NOT NULL
);

-- Module dependencies (imports)
CREATE TABLE IF NOT EXISTS dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Module that has the dependency
    module_id INTEGER NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    -- Path of the imported module
    depends_on_path TEXT NOT NULL,
    -- Import kind: 'module', 'glob', 'items'
    import_kind TEXT NOT NULL,
    -- Specific items imported (JSON array), NULL for module/glob
    imported_items TEXT
);

-- Indexes for fast lookup
CREATE INDEX IF NOT EXISTS idx_symbols_module ON symbols(module_id);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_qualified ON symbols(qualified_name);
CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
CREATE INDEX IF NOT EXISTS idx_deps_module ON dependencies(module_id);
CREATE INDEX IF NOT EXISTS idx_deps_depends_on ON dependencies(depends_on_path);
CREATE INDEX IF NOT EXISTS idx_modules_path ON modules(path);
"#;
