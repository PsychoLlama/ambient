//! Symbol database for fast LSP lookup and incremental compilation.
//!
//! This module provides a SQLite-based database that stores:
//! - Module information (path, source hash, export hash)
//! - Symbol records (functions, constants, types, enums, abilities)
//! - Type signatures in JSON format
//! - Module dependencies for invalidation tracking
//!
//! The database supports hash-based diffing: only cascade invalidation
//! to dependents when a module's export signature actually changes.

mod db;
mod export_hash;
mod extract;
mod schema;
mod serialize;

pub use db::{ModuleInfo, SymbolDb, SymbolDbError};
pub use export_hash::{compute_export_hash, compute_source_hash};
pub use extract::{extract_dependencies, extract_symbols, DependencyInfo, SymbolInfo, SymbolKind};
pub use serialize::{
    deserialize_ability_set, deserialize_type, serialize_ability_set, serialize_type,
};
