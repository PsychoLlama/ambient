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

    /// Iterate over all open document URIs.
    pub fn uris(&self) -> impl Iterator<Item = &Uri> {
        self.documents.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_uri() -> Uri {
        "file:///test.ab".parse().expect("valid uri")
    }

    #[test]
    fn test_line_offsets() {
        let text = "line1\nline2\nline3";
        let offsets = compute_line_offsets(text);
        assert_eq!(offsets, vec![0, 6, 12]);
    }

    #[test]
    fn test_line_offsets_empty() {
        let text = "";
        let offsets = compute_line_offsets(text);
        assert_eq!(offsets, vec![0]);
    }

    #[test]
    fn test_line_offsets_no_newlines() {
        let text = "single line";
        let offsets = compute_line_offsets(text);
        assert_eq!(offsets, vec![0]);
    }

    #[test]
    fn test_offset_to_position() {
        let doc = Document::new(test_uri(), 1, "fn foo() {\n  42\n}".to_string());

        // Start of file
        assert_eq!(doc.offset_to_position(0), (0, 0));

        // Start of second line
        assert_eq!(doc.offset_to_position(11), (1, 0));

        // "42" on second line
        assert_eq!(doc.offset_to_position(13), (1, 2));
    }

    #[test]
    fn test_offset_to_position_end_of_file() {
        let doc = Document::new(test_uri(), 1, "abc".to_string());
        // Offset beyond file length should clamp
        assert_eq!(doc.offset_to_position(100), (0, 3));
    }

    #[test]
    fn test_position_to_offset() {
        let doc = Document::new(test_uri(), 1, "fn foo() {\n  42\n}".to_string());

        // Start of file
        assert_eq!(doc.position_to_offset(0, 0), 0);

        // Start of second line
        assert_eq!(doc.position_to_offset(1, 0), 11);

        // "42" on second line
        assert_eq!(doc.position_to_offset(1, 2), 13);
    }

    #[test]
    fn test_position_to_offset_past_end() {
        let doc = Document::new(test_uri(), 1, "abc\ndef".to_string());
        // Line beyond file should return end
        assert_eq!(doc.position_to_offset(100, 0), 7);
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

    #[test]
    fn test_utf16_conversion_emoji() {
        // Emoji that requires surrogate pairs in UTF-16
        let line = "x 😀 y";
        // 'x' is 1 byte, ' ' is 1 byte, 😀 is 4 bytes in UTF-8
        let utf8_offset = 6; // After the emoji
        let utf16_offset = utf8_to_utf16_offset(line, utf8_offset);
        // 'x' = 1, ' ' = 1, 😀 = 2 (surrogate pair)
        assert_eq!(utf16_offset, 4);
    }

    #[test]
    fn test_document_update() {
        let mut doc = Document::new(test_uri(), 1, "initial".to_string());
        assert_eq!(doc.version, 1);
        assert_eq!(doc.text, "initial");

        doc.update(2, "updated\ncontent".to_string());
        assert_eq!(doc.version, 2);
        assert_eq!(doc.text, "updated\ncontent");
        assert_eq!(doc.line_offsets, vec![0, 8]);
    }

    #[test]
    fn test_document_store_open_close() {
        let mut store = DocumentStore::new();
        let uri = test_uri();

        assert!(store.get(&uri).is_none());

        store.open(uri.clone(), 1, "content".to_string());
        assert!(store.get(&uri).is_some());
        assert_eq!(store.get(&uri).unwrap().text, "content");

        store.close(&uri);
        assert!(store.get(&uri).is_none());
    }

    #[test]
    fn test_document_store_update() {
        let mut store = DocumentStore::new();
        let uri = test_uri();

        store.open(uri.clone(), 1, "original".to_string());
        store.update(&uri, 2, "modified".to_string());

        let doc = store.get(&uri).unwrap();
        assert_eq!(doc.version, 2);
        assert_eq!(doc.text, "modified");
    }

    #[test]
    fn test_document_store_update_nonexistent() {
        let mut store = DocumentStore::new();
        let uri = test_uri();

        // Update on non-existent document should be a no-op
        store.update(&uri, 1, "content".to_string());
        assert!(store.get(&uri).is_none());
    }

    #[test]
    fn test_document_store_multiple_documents() {
        let mut store = DocumentStore::new();
        let uri1: Uri = "file:///test1.ab".parse().unwrap();
        let uri2: Uri = "file:///test2.ab".parse().unwrap();

        store.open(uri1.clone(), 1, "doc1".to_string());
        store.open(uri2.clone(), 1, "doc2".to_string());

        assert_eq!(store.get(&uri1).unwrap().text, "doc1");
        assert_eq!(store.get(&uri2).unwrap().text, "doc2");

        store.close(&uri1);
        assert!(store.get(&uri1).is_none());
        assert!(store.get(&uri2).is_some());
    }
}
