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

use ambient_engine::ast::{ItemKind, Module, UseKind, UsePrefix};
use ambient_engine::core_library::CoreLibrary;

use crate::util::{path_to_uri, uri_to_path};

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
    /// Module-level documentation (from `//!` comments).
    pub doc: Option<Arc<str>>,
}

/// Information about a use statement.
#[derive(Debug, Clone)]
pub struct UseInfo {
    /// The prefix of the import (pkg, core, self, super).
    pub prefix: UsePrefixInfo,
    /// The module path segments with their source spans.
    pub path: Vec<PathSegment>,
    /// What is imported.
    pub kind: UseKindInfo,
    /// Byte offset of the use statement.
    pub offset: u32,
}

/// A path segment in a use statement.
#[derive(Debug, Clone)]
pub struct PathSegment {
    /// The segment name.
    pub name: Arc<str>,
    /// The byte offset where this segment starts.
    pub start: u32,
    /// The byte offset where this segment ends.
    pub end: u32,
}

/// The prefix of a use path for the LSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePrefixInfo {
    /// `pkg.module` - Local package
    Pkg,
    /// `core.module` - Core library
    Core,
    /// `self.sibling` - Same directory
    Self_,
    /// `super.module` - Parent directory with level count
    Super(usize),
}

impl From<&UsePrefix> for UsePrefixInfo {
    fn from(prefix: &UsePrefix) -> Self {
        match prefix {
            UsePrefix::Pkg => Self::Pkg,
            UsePrefix::Core => Self::Core,
            UsePrefix::Self_ => Self::Self_,
            UsePrefix::Super(n) => Self::Super(*n),
        }
    }
}

/// What is imported from a use path.
#[derive(Debug, Clone)]
pub enum UseKindInfo {
    /// Import the module itself: `use pkg.module;`.
    Module,
    /// Import specific items: `use pkg.module.{a, b}`.
    Items(Vec<Arc<str>>),
}

impl From<&UseKind> for UseKindInfo {
    fn from(kind: &UseKind) -> Self {
        match kind {
            UseKind::Module => Self::Module,
            UseKind::Items(items) => Self::Items(items.clone()),
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
            doc: module.doc.clone(),
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

    /// Find the module referenced at a given cursor position in a use statement.
    ///
    /// Given a file URI and cursor offset, checks if the cursor is within a path
    /// segment of a use statement, and if so, resolves that module.
    #[must_use]
    pub fn find_use_module_at_offset(&self, current_uri: &Uri, offset: u32) -> Option<&ModuleInfo> {
        let current_module = self.modules.get(current_uri.as_str())?;

        for use_info in &current_module.uses {
            // Find which path segment the cursor is in
            for (idx, segment) in use_info.path.iter().enumerate() {
                if offset >= segment.start && offset < segment.end {
                    // Cursor is within this segment - resolve the partial path
                    let partial_path: Vec<_> = use_info
                        .path
                        .iter()
                        .take(idx + 1)
                        .map(|s| s.name.clone())
                        .collect();

                    // Resolve based on prefix
                    let resolved_path = self.resolve_partial_use_path(
                        current_uri,
                        &use_info.prefix,
                        &partial_path,
                    )?;

                    return self.find_module(&resolved_path);
                }
            }
        }

        None
    }

    /// Resolve a partial use path based on prefix.
    fn resolve_partial_use_path(
        &self,
        current_uri: &Uri,
        prefix: &UsePrefixInfo,
        partial_path: &[Arc<str>],
    ) -> Option<Vec<Arc<str>>> {
        match prefix {
            UsePrefixInfo::Pkg => {
                // Absolute path from package root
                Some(partial_path.to_vec())
            }
            UsePrefixInfo::Core => {
                // Core library - not in workspace index
                None
            }
            UsePrefixInfo::Self_ => {
                // Relative to current module's parent directory
                let current_module = self.modules.get(current_uri.as_str())?;
                let mut resolved = current_module.module_path.clone();
                // Remove the current module name
                resolved.pop();
                // Add the import path
                resolved.extend(partial_path.iter().cloned());
                Some(resolved)
            }
            UsePrefixInfo::Super(levels) => {
                // Go up n levels from current module
                let current_module = self.modules.get(current_uri.as_str())?;
                let mut resolved = current_module.module_path.clone();
                // Remove the current module name
                resolved.pop();
                // Go up additional levels
                for _ in 0..*levels {
                    resolved.pop();
                }
                // Add the import path
                resolved.extend(partial_path.iter().cloned());
                Some(resolved)
            }
        }
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
            // Skip core library imports for now (they're not in the workspace index)
            if matches!(use_info.prefix, UsePrefixInfo::Core) {
                continue;
            }

            // Resolve the actual path based on prefix
            let resolved_path = self.resolve_use_path(current_uri, use_info)?;

            match &use_info.kind {
                UseKindInfo::Module => {
                    // Check if this module exports the name
                    if let Some(result) = self.find_symbol(&resolved_path, name) {
                        return Some(result);
                    }
                }
                UseKindInfo::Items(items) => {
                    // Check if this specific name is imported
                    if items.iter().any(|i| i.as_ref() == name)
                        && let Some(result) = self.find_symbol(&resolved_path, name)
                    {
                        return Some(result);
                    }
                }
            }
        }

        None
    }

    /// Resolve a use statement path based on its prefix and the current file.
    fn resolve_use_path(&self, current_uri: &Uri, use_info: &UseInfo) -> Option<Vec<Arc<str>>> {
        let path_names: Vec<_> = use_info.path.iter().map(|s| s.name.clone()).collect();

        match use_info.prefix {
            UsePrefixInfo::Pkg => {
                // Absolute path from package root
                Some(path_names)
            }
            UsePrefixInfo::Core => {
                // Core library - not in workspace index
                None
            }
            UsePrefixInfo::Self_ => {
                // Relative to current module's parent directory
                let current_module = self.modules.get(current_uri.as_str())?;
                let mut resolved = current_module.module_path.clone();
                // Remove the current module name
                resolved.pop();
                // Add the import path
                resolved.extend(path_names);
                Some(resolved)
            }
            UsePrefixInfo::Super(levels) => {
                // Go up n levels from current module
                let current_module = self.modules.get(current_uri.as_str())?;
                let mut resolved = current_module.module_path.clone();
                // Remove the current module name
                resolved.pop();
                // Go up additional levels
                for _ in 0..levels {
                    resolved.pop();
                }
                // Add the import path
                resolved.extend(path_names);
                Some(resolved)
            }
        }
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
                return path_to_uri(&file_path);
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
        let Some(file_path) = uri_to_path(uri) else {
            return Vec::new();
        };

        // If we have a workspace root, compute relative path
        if let Some(root) = &self.workspace_root
            && let Ok(relative) = file_path.strip_prefix(root)
        {
            return path_to_module_segments(relative);
        }

        // Fall back to just the file stem
        if let Some(stem) = file_path.file_stem()
            && let Some(name) = stem.to_str()
        {
            return vec![Arc::from(name)];
        }

        Vec::new()
    }

    /// Get all indexed modules.
    pub fn all_modules(&self) -> impl Iterator<Item = &ModuleInfo> {
        self.modules.values()
    }

    /// Check if a use info refers to a core library import.
    #[must_use]
    pub fn is_core_import(use_info: &UseInfo) -> bool {
        matches!(use_info.prefix, UsePrefixInfo::Core)
    }

    /// Get the available core library modules.
    #[must_use]
    pub fn available_core_modules() -> Vec<&'static str> {
        CoreLibrary::available_modules()
    }

    /// Check if a core module path exists.
    #[must_use]
    pub fn core_module_exists(path: &[Arc<str>]) -> bool {
        CoreLibrary::has_module(path)
    }
}

/// Convert a file path to module segments.
///
/// E.g., `src/utils/math.ab` -> `["src", "utils", "math"]`
fn path_to_module_segments(path: &Path) -> Vec<Arc<str>> {
    let mut segments = Vec::new();

    for component in path.components() {
        if let std::path::Component::Normal(s) = component
            && let Some(name) = s.to_str()
        {
            // Remove .ab extension from last component
            let name = name.strip_suffix(".ab").unwrap_or(name);
            segments.push(Arc::from(name));
        }
    }

    segments
}

/// Extract exported symbols from a module.
fn extract_exports(module: &Module) -> Vec<ExportedSymbol> {
    let mut exports = Vec::new();

    for item in &module.items {
        let (name, kind, name_span, is_public) = match &item.kind {
            ItemKind::Function(f) => (
                f.name.clone(),
                SymbolKind::Function,
                f.name_span,
                f.is_public,
            ),
            ItemKind::Const(c) => (c.name.clone(), SymbolKind::Const, c.name_span, true),
            ItemKind::TypeAlias(t) => (t.name.clone(), SymbolKind::TypeAlias, t.name_span, true),
            ItemKind::Enum(e) => (e.name.clone(), SymbolKind::Enum, e.name_span, true),
            ItemKind::Ability(a) => (a.name.clone(), SymbolKind::Ability, a.name_span, true),
            ItemKind::Trait(t) => (t.name.clone(), SymbolKind::Ability, t.name_span, true),
            ItemKind::Impl(_) | ItemKind::Use(_) => continue,
        };

        // Only export public items (or all for now until we have proper visibility)
        if is_public || !matches!(item.kind, ItemKind::Function(_)) {
            exports.push(ExportedSymbol {
                name,
                kind,
                offset: name_span.start,
                end_offset: name_span.end,
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
                prefix: UsePrefixInfo::from(&use_def.prefix),
                path: use_def
                    .path
                    .iter()
                    .map(|(name, span)| PathSegment {
                        name: name.clone(),
                        start: span.start,
                        end: span.end,
                    })
                    .collect(),
                kind: UseKindInfo::from(&use_def.kind),
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
                name_span: span,
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
            doc: None,
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
            doc: None,
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
            doc: None,
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

    #[test]
    fn test_use_prefix_info_from() {
        assert_eq!(UsePrefixInfo::from(&UsePrefix::Pkg), UsePrefixInfo::Pkg);
        assert_eq!(UsePrefixInfo::from(&UsePrefix::Core), UsePrefixInfo::Core);
        assert_eq!(UsePrefixInfo::from(&UsePrefix::Self_), UsePrefixInfo::Self_);
        assert_eq!(
            UsePrefixInfo::from(&UsePrefix::Super(2)),
            UsePrefixInfo::Super(2)
        );
    }

    #[test]
    fn test_is_core_import() {
        let core_use = UseInfo {
            prefix: UsePrefixInfo::Core,
            path: vec![PathSegment {
                name: Arc::from("List"),
                start: 0,
                end: 4,
            }],
            kind: UseKindInfo::Module,
            offset: 0,
        };
        let pkg_use = UseInfo {
            prefix: UsePrefixInfo::Pkg,
            path: vec![PathSegment {
                name: Arc::from("utils"),
                start: 0,
                end: 5,
            }],
            kind: UseKindInfo::Module,
            offset: 0,
        };

        assert!(WorkspaceIndex::is_core_import(&core_use));
        assert!(!WorkspaceIndex::is_core_import(&pkg_use));
    }

    #[test]
    fn test_available_core_modules() {
        let modules = WorkspaceIndex::available_core_modules();
        assert!(modules.contains(&"List"));
        assert!(modules.contains(&"string"));
        assert!(modules.contains(&"math"));
    }

    #[test]
    fn test_core_module_exists() {
        assert!(WorkspaceIndex::core_module_exists(&[Arc::from("List")]));
        assert!(WorkspaceIndex::core_module_exists(&[Arc::from("math")]));
        assert!(!WorkspaceIndex::core_module_exists(&[Arc::from(
            "nonexistent"
        )]));
    }
}
