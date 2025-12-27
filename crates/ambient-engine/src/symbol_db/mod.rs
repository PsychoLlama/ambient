//! Symbol database for bidirectional hash ↔ symbol path lookups.
//!
//! This module provides a SQLite-based database that stores:
//! - Symbol path → hash mappings (bidirectional)
//! - Lambda parent tracking (for navigating to anonymous functions)
//! - Type registry (for hover/completions)
//! - Hash-level dependencies (for find references)
//!
//! Spans are not stored - they are computed dynamically by parsing
//! source files at lookup time.

mod db;
mod schema;
mod serialize;

pub use db::{
    CleanupStats, DependencyKind, SymbolDb, SymbolDbError, SymbolKind, SymbolPathEntry, TypeEntry,
    TypeKind,
};
pub use serialize::{
    deserialize_ability_set, deserialize_type, serialize_ability_set, serialize_type,
};
