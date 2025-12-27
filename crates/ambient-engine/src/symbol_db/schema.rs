//! SQL schema definitions for the symbol database.

/// SQL to create all tables.
pub const CREATE_SCHEMA: &str = r#"
-- Hash registry (normalized storage)
CREATE TABLE IF NOT EXISTS hashes (
    id INTEGER PRIMARY KEY,
    hash BLOB UNIQUE NOT NULL
);

-- Symbol path -> Hash mapping
CREATE TABLE IF NOT EXISTS symbol_paths (
    path TEXT PRIMARY KEY,           -- e.g., "mylib.utils.format"
    kind TEXT NOT NULL,              -- 'function', 'const', 'enum', 'ability'
    module_path TEXT NOT NULL,       -- for cleanup by module
    hash_id INTEGER NOT NULL REFERENCES hashes(id)
);

-- Lambda parent mapping (lambdas don't have symbol paths)
CREATE TABLE IF NOT EXISTS lambda_parents (
    hash_id INTEGER PRIMARY KEY REFERENCES hashes(id) ON DELETE CASCADE,
    parent_path TEXT NOT NULL        -- Parent function's symbol path
);

-- Type registry (separate namespace)
CREATE TABLE IF NOT EXISTS types (
    path TEXT PRIMARY KEY,           -- "mylib.foo.MyType" or "mylib.foo.bar:param:0"
    kind TEXT NOT NULL,              -- 'named', 'enum', 'alias', 'anonymous_record', 'anonymous_tuple'
    module_path TEXT NOT NULL,
    type_hash BLOB NOT NULL,
    signature TEXT NOT NULL,         -- JSON serialized type (needed for hover/completions)
    parent_symbol TEXT               -- For anonymous types: parent symbol's path
);

-- Hash-level dependencies (for find references)
CREATE TABLE IF NOT EXISTS hash_dependencies (
    dependent_hash_id INTEGER NOT NULL REFERENCES hashes(id) ON DELETE CASCADE,
    dependency_hash_id INTEGER NOT NULL REFERENCES hashes(id) ON DELETE CASCADE,
    kind TEXT NOT NULL DEFAULT 'call',
    PRIMARY KEY (dependent_hash_id, dependency_hash_id, kind)
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_sympath_module ON symbol_paths(module_path);
CREATE INDEX IF NOT EXISTS idx_sympath_hash ON symbol_paths(hash_id);
CREATE INDEX IF NOT EXISTS idx_lambda_parent ON lambda_parents(parent_path);
CREATE INDEX IF NOT EXISTS idx_types_module ON types(module_path);
CREATE INDEX IF NOT EXISTS idx_types_hash ON types(type_hash);
CREATE INDEX IF NOT EXISTS idx_deps_dependency ON hash_dependencies(dependency_hash_id);
"#;
