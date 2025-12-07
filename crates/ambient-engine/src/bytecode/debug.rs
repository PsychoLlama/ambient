//! Debug information for compiled bytecode.
//!
//! Provides structures for mapping bytecode locations back to source code
//! for error messages and stack traces.

use std::collections::HashMap;

/// Debug information for a compiled function.
///
/// This allows mapping bytecode locations back to source code for
/// error messages and stack traces.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    /// Path to the source file (if available).
    pub source_file: Option<String>,

    /// Name of the function (for display in stack traces).
    pub function_name: Option<String>,

    /// Maps bytecode offsets to source spans.
    ///
    /// Each entry maps a bytecode byte offset to a source span.
    /// The spans refer to byte offsets in the source file.
    pub source_map: Vec<SourceMapping>,

    /// Maps local variable slots to their names.
    ///
    /// This helps with debugging output by showing meaningful variable names.
    pub local_names: HashMap<u16, String>,
}

/// A single mapping from bytecode offset to source location.
#[derive(Debug, Clone)]
pub struct SourceMapping {
    /// Byte offset in the bytecode.
    pub bytecode_offset: usize,

    /// Start byte offset in the source file.
    pub source_start: usize,

    /// End byte offset in the source file.
    pub source_end: usize,

    /// Line number (1-indexed) in the source file.
    pub line: u32,

    /// Column number (1-indexed) in the source file.
    pub column: u32,
}

impl DebugInfo {
    /// Create empty debug info.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create debug info with a source file and function name.
    #[must_use]
    pub fn with_source(source_file: impl Into<String>, function_name: impl Into<String>) -> Self {
        Self {
            source_file: Some(source_file.into()),
            function_name: Some(function_name.into()),
            ..Self::default()
        }
    }

    /// Add a source mapping.
    pub fn add_mapping(
        &mut self,
        bytecode_offset: usize,
        source_start: usize,
        source_end: usize,
        line: u32,
        column: u32,
    ) {
        self.source_map.push(SourceMapping {
            bytecode_offset,
            source_start,
            source_end,
            line,
            column,
        });
    }

    /// Add a local variable name mapping.
    pub fn add_local_name(&mut self, slot: u16, name: impl Into<String>) {
        self.local_names.insert(slot, name.into());
    }

    /// Find the source location for a bytecode offset.
    ///
    /// Returns the mapping with the highest bytecode offset that is <= the given offset.
    #[must_use]
    pub fn find_source_location(&self, bytecode_offset: usize) -> Option<&SourceMapping> {
        // Binary search for the largest offset <= bytecode_offset
        // Since mappings are added in order, we can search efficiently
        let mut result: Option<&SourceMapping> = None;
        for mapping in &self.source_map {
            if mapping.bytecode_offset <= bytecode_offset {
                result = Some(mapping);
            } else {
                break;
            }
        }
        result
    }

    /// Get the name of a local variable, if known.
    #[must_use]
    pub fn get_local_name(&self, slot: u16) -> Option<&str> {
        self.local_names.get(&slot).map(String::as_str)
    }
}
