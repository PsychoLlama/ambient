//! Fluent builder for LSP tests.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use lsp_types::{Diagnostic, Uri};
use tempfile::TempDir;

use super::assertions::{CompletionResult, DefinitionResult, HoverResult};
use super::client::TestClient;
use super::fixtures::{get_cursor_by_name, parse_markers, Cursor};

/// Fluent builder for LSP tests.
///
/// # Example
///
/// ```ignore
/// LspTest::new()
///     .with_source("fn foo() { let x/*h*/ = 42; x }")
///     .hover_at("h")
///     .expect_type("number");
/// ```
pub struct LspTest {
    /// Files in the test (path -> (content, cursors)).
    files: HashMap<String, (String, Vec<Cursor>)>,
    /// The currently active/open file.
    main_file: Option<String>,
    /// The LSP test client.
    client: Option<TestClient>,
    /// Temporary directory for multi-file tests.
    temp_dir: Option<TempDir>,
    /// Whether this is a package test (has ambient.toml).
    is_package: bool,
}

impl LspTest {
    /// Create a new LSP test builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            main_file: None,
            client: None,
            temp_dir: None,
            is_package: false,
        }
    }

    /// Set the source code for a single-file test.
    ///
    /// Cursor markers (`/*name*/` or `/*|*/`) are extracted and removed.
    #[must_use]
    pub fn with_source(mut self, source: &str) -> Self {
        let (clean, cursors) = parse_markers(source);
        self.files.insert("test.ab".to_string(), (clean, cursors));
        self.main_file = Some("test.ab".to_string());
        self
    }

    /// Enable package mode (creates ambient.toml).
    ///
    /// Use this for multi-file tests with imports.
    #[must_use]
    pub fn with_package(mut self) -> Self {
        self.is_package = true;
        self
    }

    /// Add a file to the test.
    ///
    /// For multi-file tests, use `with_package()` first.
    #[must_use]
    pub fn with_file(mut self, path: &str, content: &str) -> Self {
        let (clean, cursors) = parse_markers(content);
        self.files.insert(path.to_string(), (clean, cursors));

        // Set as main file if this is the first file
        if self.main_file.is_none() {
            self.main_file = Some(path.to_string());
        }

        self
    }

    /// Set which file is currently open/active for queries.
    #[must_use]
    pub fn open_file(mut self, path: &str) -> Self {
        self.main_file = Some(path.to_string());
        self
    }

    /// Ensure the client is initialized and files are opened.
    fn ensure_initialized(&mut self) {
        if self.client.is_some() {
            return;
        }

        // Create temp directory for files
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let root = temp_dir.path();

        // If this is a package, create ambient.toml
        if self.is_package {
            let manifest = r#"[package]
name = "test"
version = "0.1.0"

[source]
path = "src"
"#;
            fs::write(root.join("ambient.toml"), manifest).expect("Failed to write ambient.toml");
        }

        // Write all files
        for (path, (content, _)) in &self.files {
            let full_path = root.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).expect("Failed to create directories");
            }
            fs::write(&full_path, content).expect("Failed to write file");
        }

        // Create client and open main file
        let mut client = TestClient::new();

        // Open the main file
        if let Some(main_file) = &self.main_file {
            let full_path = root.join(main_file);
            let uri = path_to_uri(&full_path);
            let (content, _) = self.files.get(main_file).expect("Main file not found");
            client.open_document(uri, content);
        }

        self.client = Some(client);
        self.temp_dir = Some(temp_dir);
    }

    /// Get the URI for the main file.
    fn main_uri(&self) -> Uri {
        let root = self.temp_dir.as_ref().expect("Not initialized").path();
        let main_file = self.main_file.as_ref().expect("No main file");
        path_to_uri(&root.join(main_file))
    }

    /// Get all cursors across all files.
    fn all_cursors(&self) -> Vec<(String, Cursor)> {
        self.files
            .iter()
            .flat_map(|(path, (_, cursors))| cursors.iter().map(|c| (path.clone(), c.clone())))
            .collect()
    }

    /// Get a cursor by name from the main file.
    fn get_cursor(&self, name: &str) -> &Cursor {
        let main_file = self.main_file.as_ref().expect("No main file");
        let (_, cursors) = self.files.get(main_file).expect("Main file not found");
        get_cursor_by_name(cursors, name).unwrap_or_else(|| {
            panic!(
                "Cursor '{}' not found in {}. Available: {:?}",
                name,
                main_file,
                cursors.iter().map(|c| &c.name).collect::<Vec<_>>()
            )
        })
    }

    // -------------------------------------------------------------------------
    // Diagnostics
    // -------------------------------------------------------------------------

    /// Assert that a diagnostic exists at the given line (1-indexed) containing the message.
    #[must_use]
    pub fn expect_diagnostic_at(mut self, line: u32, msg: &str) -> Self {
        self.ensure_initialized();

        let uri = self.main_uri();
        let client = self.client.as_ref().unwrap();
        let diagnostics = client.get_diagnostics(&uri);

        let found = diagnostics
            .iter()
            .any(|d| d.range.start.line == line - 1 && d.message.contains(msg));

        assert!(
            found,
            "Expected diagnostic at line {} containing '{}', but got: {:?}",
            line,
            msg,
            diagnostics
                .iter()
                .map(|d| format!("L{}: {}", d.range.start.line + 1, d.message))
                .collect::<Vec<_>>()
        );

        self
    }

    /// Assert that no diagnostics are present.
    #[must_use]
    pub fn expect_no_diagnostics(mut self) -> Self {
        self.ensure_initialized();

        let uri = self.main_uri();
        let client = self.client.as_ref().unwrap();
        let diagnostics = client.get_diagnostics(&uri);

        assert!(
            diagnostics.is_empty(),
            "Expected no diagnostics, but got: {:?}",
            diagnostics
                .iter()
                .map(|d| format!("L{}: {}", d.range.start.line + 1, d.message))
                .collect::<Vec<_>>()
        );

        self
    }

    /// Assert the exact number of diagnostics.
    #[must_use]
    pub fn expect_diagnostic_count(mut self, n: usize) -> Self {
        self.ensure_initialized();

        let uri = self.main_uri();
        let client = self.client.as_ref().unwrap();
        let diagnostics = client.get_diagnostics(&uri);

        assert_eq!(
            diagnostics.len(),
            n,
            "Expected {} diagnostics, but got {}: {:?}",
            n,
            diagnostics.len(),
            diagnostics
                .iter()
                .map(|d| format!("L{}: {}", d.range.start.line + 1, d.message))
                .collect::<Vec<_>>()
        );

        self
    }

    /// Get the raw diagnostics for custom assertions.
    pub fn get_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.ensure_initialized();
        let uri = self.main_uri();
        let client = self.client.as_ref().unwrap();
        client.get_diagnostics(&uri)
    }

    // -------------------------------------------------------------------------
    // Hover
    // -------------------------------------------------------------------------

    /// Request hover at the named cursor position.
    #[must_use]
    pub fn hover_at(mut self, cursor_name: &str) -> HoverResult {
        self.ensure_initialized();

        let cursor = self.get_cursor(cursor_name).clone();
        let uri = self.main_uri();
        let client = self.client.as_mut().unwrap();

        let hover = client.hover(&uri, cursor.line, cursor.character);

        HoverResult { test: self, hover }
    }

    /// Request hover at an explicit position.
    #[must_use]
    pub fn hover_at_pos(mut self, line: u32, character: u32) -> HoverResult {
        self.ensure_initialized();

        let uri = self.main_uri();
        let client = self.client.as_mut().unwrap();

        let hover = client.hover(&uri, line, character);

        HoverResult { test: self, hover }
    }

    // -------------------------------------------------------------------------
    // Go-to-definition
    // -------------------------------------------------------------------------

    /// Request go-to-definition at the named cursor position.
    #[must_use]
    pub fn goto_definition_at(mut self, cursor_name: &str) -> DefinitionResult {
        self.ensure_initialized();

        let cursor = self.get_cursor(cursor_name).clone();
        let uri = self.main_uri();
        let client = self.client.as_mut().unwrap();

        let locations = client.goto_definition(&uri, cursor.line, cursor.character);
        let all_cursors = self.all_cursors();

        DefinitionResult {
            test: self,
            locations,
            all_cursors,
        }
    }

    // -------------------------------------------------------------------------
    // Completions
    // -------------------------------------------------------------------------

    /// Request completions at the named cursor position.
    #[must_use]
    pub fn complete_at(mut self, cursor_name: &str) -> CompletionResult {
        self.ensure_initialized();

        let cursor = self.get_cursor(cursor_name).clone();
        let uri = self.main_uri();
        let client = self.client.as_mut().unwrap();

        let items = client.complete(&uri, cursor.line, cursor.character);

        CompletionResult { test: self, items }
    }

    /// Shutdown the test client.
    ///
    /// This is called automatically when the test is dropped, but can be called
    /// explicitly for cleaner test output.
    pub fn shutdown(mut self) {
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
    }
}

impl Default for LspTest {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LspTest {
    fn drop(&mut self) {
        // Don't try to shutdown if we're already panicking
        if std::thread::panicking() {
            return;
        }
        // Shutdown client if it exists
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
    }
}

/// Convert a path to a file:// URI.
fn path_to_uri(path: &PathBuf) -> Uri {
    let path_str = path.to_string_lossy();
    let uri_str = format!("file://{}", path_str);
    uri_str.parse().expect("Failed to parse URI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_single_file() {
        LspTest::new()
            .with_source("fn foo() { 42 }")
            .expect_no_diagnostics()
            .shutdown();
    }

    #[test]
    fn test_builder_with_markers() {
        let test = LspTest::new().with_source("fn foo() { let x/*h*/ = 42; x }");

        // Verify markers are parsed
        let (_, cursors) = test.files.get("test.ab").unwrap();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].name, "h");
    }
}
