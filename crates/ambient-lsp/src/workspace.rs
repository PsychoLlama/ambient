//! Workspace-level indexing for cross-file navigation.
//!
//! This module provides the [`WorkspaceIndex`] which tracks:
//! - Module paths to file URIs
//! - Exported symbols from each module
//! - Use/import relationships between files
//!
//! This enables cross-file go-to-definition and other multi-file features.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::Uri;

use ambient_engine::ast::{ItemKind, Module, UseImports};

/// Information about a symbol exported from a module.
#[derive(Debug, Clone)]
pub struct ExportedSymbol {
    /// The symbol name.
    pub name: Arc<str>,
    /// The kind of symbol.
    pub kind: SymbolKind,
    /// Byte offset of the definition in the source.
    pub offset: u32,
    /// End byte offset of the definition.
    pub end_offset: u32,
}

/// The kind of exported symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Const,
    TypeAlias,
    Enum,
    Ability,
}

/// Information about a single module/file in the workspace.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    /// The file URI.
    pub uri: Uri,
    /// The module path (e.g., `["std", "io"]` for `std/io.ab`).
    pub module_path: Vec<Arc<str>>,
    /// Symbols exported by this module.
    pub exports: Vec<ExportedSymbol>,
    /// Use statements in this module (for resolving imports).
    pub uses: Vec<UseInfo>,
}

/// Information about a use statement.
#[derive(Debug, Clone)]
pub struct UseInfo {
    /// The module path being imported.
    pub path: Vec<Arc<str>>,
    /// What is imported.
    pub imports: UseImportsInfo,
    /// Byte offset of the use statement.
    pub offset: u32,
}

/// What is imported from a use path.
#[derive(Debug, Clone)]
pub enum UseImportsInfo {
    /// Import everything: `use module.*`.
    All,
    /// Import specific items: `use module.{a, b}`.
    Items(Vec<Arc<str>>),
}

impl From<&UseImports> for UseImportsInfo {
    fn from(imports: &UseImports) -> Self {
        match imports {
            UseImports::All => Self::All,
            UseImports::Items(items) => Self::Items(items.clone()),
        }
    }
}

/// Index of all modules in a workspace.
///
/// Tracks the mapping from module paths to files and their exported symbols,
/// enabling cross-file navigation.
#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    /// Map from file URI (as string) to module info.
    modules: HashMap<String, ModuleInfo>,
    /// Map from module path to file URI.
    /// The key is the module path joined with ".".
    path_to_uri: HashMap<String, Uri>,
    /// The workspace root directory (for resolving relative paths).
    workspace_root: Option<PathBuf>,
}

impl WorkspaceIndex {
    /// Create a new empty workspace index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the workspace root directory.
    pub fn set_workspace_root(&mut self, root: PathBuf) {
        self.workspace_root = Some(root);
    }

    /// Get the workspace root directory.
    #[must_use]
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Update the index for a file with the given analyzed module.
    pub fn update(&mut self, uri: Uri, module: &Module) {
        let uri_str = uri.as_str().to_string();

        // Compute module path from URI
        let module_path = self.uri_to_module_path(&uri);

        // Extract exports from the module
        let exports = extract_exports(module);

        // Extract use statements
        let uses = extract_uses(module);

        // Store the module info
        let info = ModuleInfo {
            uri: uri.clone(),
            module_path: module_path.clone(),
            exports,
            uses,
        };

        self.modules.insert(uri_str, info);

        // Update path-to-uri mapping
        let path_key = module_path
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(".");
        if !path_key.is_empty() {
            self.path_to_uri.insert(path_key, uri);
        }
    }

    /// Remove a file from the index.
    pub fn remove(&mut self, uri: &Uri) {
        let uri_str = uri.as_str();

        if let Some(info) = self.modules.remove(uri_str) {
            // Remove from path mapping
            let path_key = info
                .module_path
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>()
                .join(".");
            self.path_to_uri.remove(&path_key);
        }
    }

    /// Look up a module by its path.
    #[must_use]
    pub fn find_module(&self, path: &[Arc<str>]) -> Option<&ModuleInfo> {
        let path_key = path.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(".");
        let uri = self.path_to_uri.get(&path_key)?;
        self.modules.get(uri.as_str())
    }

    /// Look up a symbol in a module.
    #[must_use]
    pub fn find_symbol(
        &self,
        module_path: &[Arc<str>],
        name: &str,
    ) -> Option<(&ModuleInfo, &ExportedSymbol)> {
        let module = self.find_module(module_path)?;
        let symbol = module.exports.iter().find(|e| e.name.as_ref() == name)?;
        Some((module, symbol))
    }

    /// Resolve a qualified name to a definition location.
    ///
    /// Given a name with optional path prefix (e.g., `math.sin` or just `sin`),
    /// and the current file's URI, resolve to the defining module and symbol.
    #[must_use]
    pub fn resolve_name(
        &self,
        current_uri: &Uri,
        path: &[Arc<str>],
        name: &str,
    ) -> Option<(&ModuleInfo, &ExportedSymbol)> {
        // If path is provided, look up directly
        if !path.is_empty() {
            return self.find_symbol(path, name);
        }

        // Otherwise, check imports in the current file
        let current_module = self.modules.get(current_uri.as_str())?;

        for use_info in &current_module.uses {
            match &use_info.imports {
                UseImportsInfo::All => {
                    // Check if this module exports the name
                    if let Some(result) = self.find_symbol(&use_info.path, name) {
                        return Some(result);
                    }
                }
                UseImportsInfo::Items(items) => {
                    // Check if this specific name is imported
                    if items.iter().any(|i| i.as_ref() == name) {
                        if let Some(result) = self.find_symbol(&use_info.path, name) {
                            return Some(result);
                        }
                    }
                }
            }
        }

        None
    }

    /// Resolve a module path to a file URI.
    ///
    /// This handles both indexed modules and attempts to find files on disk.
    #[must_use]
    pub fn resolve_module_uri(&self, path: &[Arc<str>]) -> Option<Uri> {
        // First, check if we have it indexed
        let path_key = path.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(".");
        if let Some(uri) = self.path_to_uri.get(&path_key) {
            return Some(uri.clone());
        }

        // Try to find the file on disk relative to workspace root
        if let Some(root) = &self.workspace_root {
            let mut file_path = root.clone();
            for segment in path {
                file_path.push(segment.as_ref());
            }
            file_path.set_extension("ab");

            if file_path.exists() {
                // Convert to URI
                return file_path_to_uri(&file_path);
            }
        }

        None
    }

    /// Get module info for a URI.
    #[must_use]
    pub fn get_module(&self, uri: &Uri) -> Option<&ModuleInfo> {
        self.modules.get(uri.as_str())
    }

    /// Convert a file URI to a module path.
    fn uri_to_module_path(&self, uri: &Uri) -> Vec<Arc<str>> {
        let Some(file_path) = uri_to_file_path(uri) else {
            return Vec::new();
        };

        // If we have a workspace root, compute relative path
        if let Some(root) = &self.workspace_root {
            if let Ok(relative) = file_path.strip_prefix(root) {
                return path_to_module_segments(relative);
            }
        }

        // Fall back to just the file stem
        if let Some(stem) = file_path.file_stem() {
            if let Some(name) = stem.to_str() {
                return vec![Arc::from(name)];
            }
        }

        Vec::new()
    }

    /// Get all indexed modules.
    pub fn all_modules(&self) -> impl Iterator<Item = &ModuleInfo> {
        self.modules.values()
    }
}

/// Convert a file:// URI to a file path.
fn uri_to_file_path(uri: &Uri) -> Option<PathBuf> {
    let uri_str = uri.as_str();

    // Only handle file:// URIs
    if !uri_str.starts_with("file://") {
        return None;
    }

    // Extract the path part (after file://)
    // On Unix: file:///path/to/file -> /path/to/file
    // On Windows: file:///C:/path/to/file -> C:/path/to/file
    let path_str = uri_str.strip_prefix("file://")?;

    // Handle percent-encoding in URIs
    let decoded = percent_decode(path_str);

    Some(PathBuf::from(decoded))
}

/// Convert a file path to a file:// URI.
fn file_path_to_uri(path: &Path) -> Option<Uri> {
    let path_str = path.to_str()?;

    // Percent-encode the path for URI
    let encoded = percent_encode(path_str);

    let uri_str = format!("file://{encoded}");
    uri_str.parse().ok()
}

/// Decode percent-encoded characters in a URI path.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            // Try to decode a percent-encoded byte
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            // If decoding failed, keep the original
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }

    result
}

/// Percent-encode special characters in a path for URI.
fn percent_encode(s: &str) -> String {
    use std::fmt::Write;

    let mut result = String::with_capacity(s.len());

    for c in s.chars() {
        match c {
            // Reserved characters that should be encoded
            ' ' => result.push_str("%20"),
            '#' => result.push_str("%23"),
            '?' => result.push_str("%3F"),
            // Keep common path characters as-is
            '/' | ':' | '-' | '_' | '.' | '~' => result.push(c),
            // Keep alphanumeric characters as-is
            c if c.is_ascii_alphanumeric() => result.push(c),
            // Encode everything else
            c => {
                for byte in c.to_string().as_bytes() {
                    // Using write! is recommended over format! for appending
                    let _ = write!(result, "%{byte:02X}");
                }
            }
        }
    }

    result
}

/// Convert a file path to module segments.
///
/// E.g., `src/utils/math.ab` -> `["src", "utils", "math"]`
fn path_to_module_segments(path: &Path) -> Vec<Arc<str>> {
    let mut segments = Vec::new();

    for component in path.components() {
        if let std::path::Component::Normal(s) = component {
            if let Some(name) = s.to_str() {
                // Remove .ab extension from last component
                let name = name.strip_suffix(".ab").unwrap_or(name);
                segments.push(Arc::from(name));
            }
        }
    }

    segments
}

/// Extract exported symbols from a module.
fn extract_exports(module: &Module) -> Vec<ExportedSymbol> {
    let mut exports = Vec::new();

    for item in &module.items {
        let (name, kind, is_public) = match &item.kind {
            ItemKind::Function(f) => (f.name.clone(), SymbolKind::Function, f.is_public),
            ItemKind::Const(c) => (c.name.clone(), SymbolKind::Const, true), // consts are always public for now
            ItemKind::TypeAlias(t) => (t.name.clone(), SymbolKind::TypeAlias, true),
            ItemKind::Enum(e) => (e.name.clone(), SymbolKind::Enum, true),
            ItemKind::Ability(a) => (a.name.clone(), SymbolKind::Ability, true),
            ItemKind::Use(_) => continue,
        };

        // Only export public items (or all for now until we have proper visibility)
        if is_public || !matches!(item.kind, ItemKind::Function(_)) {
            exports.push(ExportedSymbol {
                name,
                kind,
                offset: item.span.start,
                end_offset: item.span.end,
            });
        }
    }

    exports
}

/// Extract use statements from a module.
fn extract_uses(module: &Module) -> Vec<UseInfo> {
    let mut uses = Vec::new();

    for item in &module.items {
        if let ItemKind::Use(use_def) = &item.kind {
            uses.push(UseInfo {
                path: use_def.path.clone(),
                imports: UseImportsInfo::from(&use_def.imports),
                offset: item.span.start,
            });
        }
    }

    uses
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_engine::ast::{FunctionDef, Item, Span};

    fn test_uri(name: &str) -> Uri {
        format!("file:///workspace/{name}.ab")
            .parse()
            .expect("valid uri")
    }

    fn make_function(name: &str, is_public: bool, span: Span) -> Item {
        Item::new(
            ItemKind::Function(FunctionDef {
                name: Arc::from(name),
                is_public,
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                abilities: vec![],
                body: ambient_engine::ast::Expr::unit(),
            }),
            span,
        )
    }

    #[test]
    fn test_update_and_find_module() {
        let mut index = WorkspaceIndex::new();
        index.set_workspace_root(PathBuf::from("/workspace"));

        let uri = test_uri("math");
        let module = Module {
            name: Arc::from("math"),
            items: vec![make_function("add", true, Span::new(0, 50))],
        };

        index.update(uri.clone(), &module);

        let info = index.get_module(&uri);
        assert!(info.is_some());
        assert_eq!(info.unwrap().exports.len(), 1);
        assert_eq!(info.unwrap().exports[0].name.as_ref(), "add");
    }

    #[test]
    fn test_find_symbol() {
        let mut index = WorkspaceIndex::new();
        index.set_workspace_root(PathBuf::from("/workspace"));

        let uri = test_uri("utils");
        let module = Module {
            name: Arc::from("utils"),
            items: vec![
                make_function("helper", true, Span::new(0, 30)),
                make_function("internal", false, Span::new(31, 60)),
            ],
        };

        index.update(uri.clone(), &module);

        // Public function should be findable
        let result = index.find_symbol(&[Arc::from("utils")], "helper");
        assert!(result.is_some());

        // Private function should not be exported
        let result = index.find_symbol(&[Arc::from("utils")], "internal");
        assert!(result.is_none());
    }

    #[test]
    fn test_remove_module() {
        let mut index = WorkspaceIndex::new();

        let uri = test_uri("temp");
        let module = Module {
            name: Arc::from("temp"),
            items: vec![],
        };

        index.update(uri.clone(), &module);
        assert!(index.get_module(&uri).is_some());

        index.remove(&uri);
        assert!(index.get_module(&uri).is_none());
    }

    #[test]
    fn test_path_to_module_segments() {
        let path = Path::new("src/utils/math.ab");
        let segments = path_to_module_segments(path);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].as_ref(), "src");
        assert_eq!(segments[1].as_ref(), "utils");
        assert_eq!(segments[2].as_ref(), "math");
    }

    #[test]
    fn test_symbol_kind() {
        assert_eq!(SymbolKind::Function, SymbolKind::Function);
        assert_ne!(SymbolKind::Function, SymbolKind::Const);
    }
}
