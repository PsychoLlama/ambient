//! Document management for the LSP server.
//!
//! Tracks open documents and their contents, enabling incremental analysis
//! and source position calculations.

use std::collections::HashMap;

use lsp_types::Uri;

/// A document opened in the editor.
#[derive(Debug, Clone)]
pub struct Document {
    /// The document URI.
    pub uri: Uri,
    /// The current version (incremented on each edit).
    pub version: i32,
    /// The document text content.
    pub text: String,
    /// Line start byte offsets (for efficient line/column conversion).
    line_offsets: Vec<usize>,
}

impl Document {
    /// Create a new document with the given content.
    #[must_use]
    pub fn new(uri: Uri, version: i32, text: String) -> Self {
        let line_offsets = compute_line_offsets(&text);
        Self {
            uri,
            version,
            text,
            line_offsets,
        }
    }

    /// Update the document content (full replacement).
    pub fn update(&mut self, version: i32, text: String) {
        self.version = version;
        self.line_offsets = compute_line_offsets(&text);
        self.text = text;
    }

    /// Convert a byte offset to a line/column position.
    ///
    /// Returns (line, column) where both are 0-indexed.
    #[must_use]
    pub fn offset_to_position(&self, offset: usize) -> (u32, u32) {
        let offset = offset.min(self.text.len());

        // Binary search to find the line containing this offset.
        let line = match self.line_offsets.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert_point) => insert_point.saturating_sub(1),
        };

        let line_start = self.line_offsets.get(line).copied().unwrap_or(0);
        let col = offset.saturating_sub(line_start);

        // Convert to UTF-16 code units for LSP.
        let col_utf16 = utf8_to_utf16_offset(&self.text[line_start..], col);

        #[allow(clippy::cast_possible_truncation)]
        (line as u32, col_utf16 as u32)
    }

    /// Convert a line/column position to a byte offset.
    ///
    /// Line and column are 0-indexed.
    #[must_use]
    pub fn position_to_offset(&self, line: u32, col: u32) -> usize {
        let line = line as usize;
        let col = col as usize;

        if line >= self.line_offsets.len() {
            return self.text.len();
        }

        let line_start = self.line_offsets[line];
        let line_end = self
            .line_offsets
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());

        let line_text = &self.text[line_start..line_end];

        // Convert UTF-16 column to UTF-8 offset.
        let byte_offset = utf16_to_utf8_offset(line_text, col);

        line_start + byte_offset.min(line_end - line_start)
    }
}

/// Compute byte offsets for the start of each line.
fn compute_line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];

    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            offsets.push(i + 1);
        }
    }

    offsets
}

/// Convert a UTF-8 byte offset within a line to UTF-16 code units.
fn utf8_to_utf16_offset(line: &str, byte_offset: usize) -> usize {
    line[..byte_offset.min(line.len())]
        .chars()
        .map(char::len_utf16)
        .sum()
}

/// Convert UTF-16 code units to a UTF-8 byte offset within a line.
fn utf16_to_utf8_offset(line: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0;
    let mut byte_offset = 0;

    for ch in line.chars() {
        if utf16_count >= utf16_offset {
            break;
        }
        utf16_count += ch.len_utf16();
        byte_offset += ch.len_utf8();
    }

    byte_offset
}

/// Store for all open documents.
#[derive(Debug, Default)]
pub struct DocumentStore {
    documents: HashMap<Uri, Document>,
}

impl DocumentStore {
    /// Create a new empty document store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new document.
    pub fn open(&mut self, uri: Uri, version: i32, text: String) {
        let doc = Document::new(uri.clone(), version, text);
        self.documents.insert(uri, doc);
    }

    /// Close a document.
    pub fn close(&mut self, uri: &Uri) {
        self.documents.remove(uri);
    }

    /// Update a document's content.
    pub fn update(&mut self, uri: &Uri, version: i32, text: String) {
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.update(version, text);
        }
    }

    /// Get a document by URI.
    #[must_use]
    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.documents.get(uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_offsets() {
        let text = "line1\nline2\nline3";
        let offsets = compute_line_offsets(text);
        assert_eq!(offsets, vec![0, 6, 12]);
    }

    #[test]
    fn test_offset_to_position() {
        let uri: Uri = "file:///test.ab".parse().expect("valid uri");
        let doc = Document::new(uri, 1, "fn foo() {\n  42\n}".to_string());

        // Start of file
        assert_eq!(doc.offset_to_position(0), (0, 0));

        // Start of second line
        assert_eq!(doc.offset_to_position(11), (1, 0));

        // "42" on second line
        assert_eq!(doc.offset_to_position(13), (1, 2));
    }

    #[test]
    fn test_position_to_offset() {
        let uri: Uri = "file:///test.ab".parse().expect("valid uri");
        let doc = Document::new(uri, 1, "fn foo() {\n  42\n}".to_string());

        // Start of file
        assert_eq!(doc.position_to_offset(0, 0), 0);

        // Start of second line
        assert_eq!(doc.position_to_offset(1, 0), 11);

        // "42" on second line
        assert_eq!(doc.position_to_offset(1, 2), 13);
    }

    #[test]
    fn test_utf16_conversion() {
        // Test with multi-byte characters.
        let line = "fn 日本語()";
        let utf8_offset = "fn 日本語".len();
        let utf16_offset = utf8_to_utf16_offset(line, utf8_offset);
        // "fn " = 3 + "日本語" = 3 chars = 6 UTF-16 code units (each is 1)
        assert_eq!(utf16_offset, 6);

        // Convert back
        let back = utf16_to_utf8_offset(line, utf16_offset);
        assert_eq!(back, utf8_offset);
    }
}
