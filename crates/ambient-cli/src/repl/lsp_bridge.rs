//! Bridge between the REPL and LSP completion service.
//!
//! This module adapts the LSP completion service for use in the REPL,
//! handling REPL-specific concerns like wrapping input in synthetic functions
//! and syncing REPL-defined symbols.

use std::path::Path;

use ambient_engine::compiler::{ReplContext, ReplItemKind};
use ambient_lsp::{CompletionService, ExternalSymbol, ExternalSymbolKind, ReplCompletion};

/// Bridge between REPL context and LSP completion service.
pub struct ReplLspBridge {
    /// The underlying completion service.
    service: CompletionService,
    /// Discovered project module paths for `pkg::` completions.
    module_paths: Vec<String>,
}

impl ReplLspBridge {
    /// Create a new REPL-LSP bridge.
    pub fn new(project_dir: &Path) -> Self {
        let module_paths = discover_modules(project_dir);
        Self {
            service: CompletionService::new(),
            module_paths,
        }
    }

    /// Sync REPL-defined symbols to the completion service.
    pub fn sync_repl_context(&mut self, ctx: &ReplContext) {
        let mut symbols = Vec::new();

        // Add user-defined functions and constants
        for (name, kind) in &ctx.item_kinds {
            let kind = match kind {
                ReplItemKind::Function => ExternalSymbolKind::Function,
                ReplItemKind::Constant => ExternalSymbolKind::Constant,
            };
            // Names are keyed internally by their `::` path (e.g.
            // `core::List::any`), which is exactly how completions surface
            // them to the user.
            symbols.push(ExternalSymbol::new(name.to_string(), kind));
        }

        self.service.set_external_symbols(symbols);
    }

    /// Get completions for REPL input.
    ///
    /// Handles REPL-specific completion logic like `pkg::` module completions
    /// and delegates to the LSP service for standard completions.
    pub fn get_completions(&mut self, line: &str, pos: usize) -> Vec<ReplCompletion> {
        // Find the word being typed (including `::` for qualified names).
        let before_cursor = &line[..pos];
        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
            .map_or(0, |i| i + 1);
        let full_prefix = &before_cursor[word_start..];

        // Check for REPL-specific completions that LSP doesn't handle
        if let Some(completions) = self.get_repl_specific_completions(full_prefix) {
            return completions;
        }

        // For standard completions, wrap in synthetic function and use LSP
        // This allows type inference to work on expressions
        let synthetic_source = format!("fn __repl__() {{ {line} }}");
        let adjusted_offset = pos + "fn __repl__() { ".len();

        self.service.update_source(&synthetic_source);
        let mut completions = self
            .service
            .get_completions(&synthetic_source, adjusted_offset);

        // Sort by priority, then alphabetically
        completions.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.label.cmp(&b.label))
        });

        completions
    }

    /// Get ghost text hint for current input.
    pub fn get_hint(&mut self, line: &str, pos: usize) -> Option<String> {
        // Only at end of line
        if pos != line.len() {
            return None;
        }

        // Find the full prefix (including `::` for qualified names like
        // "core::List").
        let word_start = line
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
            .map_or(0, |i| i + 1);
        let full_prefix = &line[word_start..];

        if full_prefix.is_empty() {
            return None;
        }

        let completions = self.get_completions(line, pos);

        // Show the best match as ghost text
        completions.first().map(|c| {
            // Completions after a `::` scope only replace the trailing segment.
            // For example, typing "core::List" gets completions like "List" (not
            // "core::List"), so match against the part after the last `::`.
            let match_prefix = if let Some(sep) = full_prefix.rfind("::") {
                &full_prefix[sep + 2..]
            } else {
                full_prefix
            };

            // Only show the suffix that would be added
            if c.replacement.starts_with(match_prefix) {
                c.replacement[match_prefix.len()..].to_string()
            } else {
                c.replacement.clone()
            }
        })
    }

    /// Get REPL-specific completions that the LSP doesn't handle.
    ///
    /// Returns `None` if standard LSP completions should be used.
    fn get_repl_specific_completions(&self, full_prefix: &str) -> Option<Vec<ReplCompletion>> {
        // Namespace paths are addressed with `::`.
        if let Some(sep) = full_prefix.rfind("::") {
            let before_scope = &full_prefix[..sep];
            let after_scope = &full_prefix[sep + 2..];

            // `pkg::` - complete project module names
            if before_scope == "pkg" {
                return Some(self.get_pkg_module_completions(after_scope));
            }

            // `pkg::module::` - complete module members (not supported yet, return empty)
            if before_scope.starts_with("pkg::") {
                // For now, we don't have module member introspection from LSP
                // This could be enhanced later
                return Some(Vec::new());
            }

            // `core::` is handled by LSP
            // Ability methods (Stdio::, etc.) are handled by LSP
        }

        None
    }

    /// Get project module completions for `pkg::` prefix.
    ///
    /// The replacement is the module path *relative* to `pkg::` (e.g. `math`,
    /// not `pkg::math`): the completer replaces from just after the trailing
    /// `::`, so the `pkg::` the user already typed must not be repeated.
    fn get_pkg_module_completions(&self, prefix: &str) -> Vec<ReplCompletion> {
        self.module_paths
            .iter()
            .filter(|p| p.starts_with(prefix))
            .map(|p| ReplCompletion {
                label: p.clone(),
                replacement: p.clone(),
                detail: Some("project module".to_string()),
                priority: 15,
            })
            .collect()
    }
}

/// Discover project modules (simplified version for REPL).
fn discover_modules(dir: &Path) -> Vec<String> {
    use ambient_engine::manifest::Manifest;

    // Find manifest by walking up
    let mut current = dir;
    let manifest_path = loop {
        let candidate = current.join("ambient.toml");
        if candidate.exists() {
            break Some(candidate);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break None,
        }
    };

    let Some(manifest_path) = manifest_path else {
        return Vec::new();
    };

    let Ok(manifest) = Manifest::from_file(&manifest_path) else {
        return Vec::new();
    };

    let project_root = manifest_path.parent().unwrap();
    let src_dir = project_root.join(&manifest.src_dir);

    if !src_dir.is_dir() {
        return Vec::new();
    }

    discover_ab_files(&src_dir, &src_dir)
}

/// Recursively discover .ab files and convert to module paths.
fn discover_ab_files(dir: &Path, src_root: &Path) -> Vec<String> {
    let mut modules = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return modules;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            modules.extend(discover_ab_files(&path, src_root));
        } else if path.extension().is_some_and(|ext| ext == "ab")
            && let Some(module_path) = path_to_module(&path, src_root)
        {
            // The root module (main.ab) maps to the empty path; it is
            // the entry point, not an importable module.
            if !module_path.is_empty() {
                modules.push(module_path);
            }
        }
    }

    modules
}

/// Convert a file path to a module path string.
fn path_to_module(path: &Path, src_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(src_root).ok()?;
    let module_path = ambient_engine::module_path::ModulePath::from_relative_file_path(relative)?;
    // Qualified module paths are addressed with `::` (e.g. `pkg::foo::bar`).
    Some(module_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_basic_completions() {
        let _bridge = ReplLspBridge::new(Path::new("/nonexistent"));
        // Basic construction test - more thorough tests in completer.rs
    }

    #[test]
    fn test_repl_specific_completions_pkg() {
        let mut bridge = ReplLspBridge::new(Path::new("/nonexistent"));
        bridge.module_paths = vec!["utils".to_string(), "math".to_string()];

        let completions = bridge.get_repl_specific_completions("pkg::");
        assert!(completions.is_some());
        let completions = completions.unwrap();
        assert_eq!(completions.len(), 2);
        assert!(completions.iter().any(|c| c.label == "utils"));
        assert!(completions.iter().any(|c| c.label == "math"));
        // Replacements are relative to `pkg::` (never dotted, never re-prefixed).
        assert!(completions.iter().all(|c| !c.replacement.contains('.')));
        assert!(
            completions
                .iter()
                .all(|c| !c.replacement.starts_with("pkg::"))
        );
    }

    #[test]
    fn test_repl_specific_completions_pkg_with_prefix() {
        let mut bridge = ReplLspBridge::new(Path::new("/nonexistent"));
        bridge.module_paths = vec!["utils".to_string(), "math".to_string()];

        let completions = bridge.get_repl_specific_completions("pkg::ma");
        assert!(completions.is_some());
        let completions = completions.unwrap();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].label, "math");
        assert_eq!(completions[0].replacement, "math");
    }

    #[test]
    fn test_repl_specific_completions_standard() {
        let bridge = ReplLspBridge::new(Path::new("/nonexistent"));

        // Standard completions should return None (use LSP)
        assert!(bridge.get_repl_specific_completions("Con").is_none());
        assert!(bridge.get_repl_specific_completions("Stdio::").is_none());
        assert!(bridge.get_repl_specific_completions("core::").is_none());
    }
}
