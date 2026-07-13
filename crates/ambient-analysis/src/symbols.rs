//! Workspace symbol search over the structured item index.
//!
//! `workspace/symbol` asks "where is every named item in this package?". The
//! answer is a decision about *what exists*, so it lives here in the analysis
//! layer, not in the LSP (which only renders the records to
//! `SymbolInformation`). Two structured indexes can answer it, and this module
//! defines how they combine:
//!
//! - **Live interfaces** — the session's per-module
//!   [`ModuleInterfaceSummary`], each carrying `structured_items` and the
//!   module's real `source_path`. This reflects the *current buffers*: every
//!   edit re-derives it.
//! - **The build snapshot** — the last build's manifest, read from the store.
//!   It reflects the last *build*, not the current buffers, so it can be stale.
//!
//! # Precedence: live always wins
//!
//! The snapshot is an *enhancement*, never a source of truth for a module the
//! session knows. A module present in the live interfaces is served entirely
//! from them; the snapshot only contributes modules the live set lacks. Since
//! the analysis session registers and analyzes the *whole* package (core,
//! platform, and every package module), the live set already covers every
//! navigable module — so today the snapshot overlay contributes nothing for a
//! normal package. It is retained as a robustness/coverage hook (a snapshot
//! module that somehow has no live counterpart still surfaces) and, more
//! importantly, to pin the precedence rule with tests so a future divergence
//! can't silently serve stale symbols.
//!
//! # Navigability
//!
//! A symbol is only useful if the editor can jump to it, which needs an
//! on-disk file. Builtin (`core`/`platform`) modules are embedded in the
//! binary with no source file, so their `source_path` is empty and they are
//! excluded — matching the LSP's long-standing behavior of listing only the
//! package's own modules.

use std::collections::BTreeMap;

use ambient_engine::disk_store::BuildManifest;
use ambient_engine::module_interface::{ItemKindTag, ModuleInterfaceSummary};

/// One workspace symbol: a top-level named item with enough to render an LSP
/// `SymbolInformation`. The `kind` stays engine-level ([`ItemKindTag`]) so the
/// analysis layer owns *what kind of thing* a symbol is; the frontend maps it
/// to its own symbol-kind enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    /// The item's own (short) name.
    pub name: String,
    /// The item's precise kind.
    pub kind: ItemKindTag,
    /// The canonical identity of the module that defines it — the natural
    /// container label (`workspace::test::utils`).
    pub module: String,
    /// The defining module's source path relative to the package `src/`
    /// directory. Always non-empty (empty-path builtins are excluded).
    pub source_path: String,
    /// The definition's byte range `(start, end)` in the module source.
    pub span: (u32, u32),
}

/// Every workspace symbol matching `query` (case-insensitive substring; an
/// empty query matches all), gathered from the live interfaces and, for
/// modules the live set does not cover, the build snapshot.
///
/// Live interfaces win per module (see the module docs). Results are sorted by
/// `(module, span, name)` for a stable presentation.
#[must_use]
pub fn workspace_symbols(
    query: &str,
    interfaces: &BTreeMap<String, ModuleInterfaceSummary>,
    snapshot: Option<&BuildManifest>,
) -> Vec<WorkspaceSymbol> {
    let needle = query.to_lowercase();
    let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);

    let mut out = Vec::new();

    // Live interfaces are truth for every module the session has analyzed.
    for (module, summary) in interfaces {
        if summary.source_path.is_empty() {
            continue; // builtin/embedded: no file to navigate to.
        }
        for item in &summary.items {
            if matches(item.name()) {
                out.push(WorkspaceSymbol {
                    name: item.name().to_string(),
                    kind: item.kind,
                    module: module.clone(),
                    source_path: summary.source_path.clone(),
                    span: item.span,
                });
            }
        }
    }

    // The snapshot only covers modules the live set lacks: live always wins.
    if let Some(manifest) = snapshot {
        for module in &manifest.modules {
            if module.source_path.is_empty() || interfaces.contains_key(&module.module) {
                continue;
            }
            for item in &module.items {
                if matches(item.name()) {
                    out.push(WorkspaceSymbol {
                        name: item.name().to_string(),
                        kind: item.kind,
                        module: module.module.clone(),
                        source_path: module.source_path.clone(),
                        span: item.span,
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| {
        a.module
            .cmp(&b.module)
            .then_with(|| a.span.cmp(&b.span))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::AnalysisPackage;
    use ambient_engine::disk_store::{ManifestItem, ManifestModule};
    use ambient_engine::module_interface::build_interfaces;
    use std::fs;
    use tempfile::TempDir;

    /// Write a package with the given `src/`-relative files (parents created).
    fn write_pkg(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().expect("temp dir");
        fs::write(
            dir.path().join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("manifest");
        let src = dir.path().join("src");
        for (name, content) in files {
            let path = src.join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, content).expect("module");
        }
        dir
    }

    fn interfaces(dir: &TempDir) -> BTreeMap<String, ModuleInterfaceSummary> {
        let pkg = AnalysisPackage::open(dir.path()).expect("open");
        build_interfaces(&pkg.build_registry())
    }

    fn names(syms: &[WorkspaceSymbol]) -> Vec<&str> {
        syms.iter().map(|s| s.name.as_str()).collect()
    }

    /// The live module key whose canonical identity ends in `suffix`.
    fn key_ending(ifaces: &BTreeMap<String, ModuleInterfaceSummary>, suffix: &str) -> String {
        ifaces
            .keys()
            .find(|k| k.ends_with(suffix))
            .unwrap_or_else(|| panic!("no live module ending in `{suffix}`"))
            .clone()
    }

    fn manifest_module(module: &str, source_path: &str, item: &str) -> ManifestModule {
        ManifestModule {
            module: module.to_string(),
            resolved_ast_hash: [0u8; 32],
            interface_hash: [0u8; 32],
            deps: vec![],
            objects: vec![],
            names: vec![],
            signatures: vec![],
            cache_key: [0u8; 32],
            consumed_links: vec![],
            migrations: vec![],
            lambda_parents: vec![],
            entry_point: None,
            source_path: source_path.to_string(),
            items: vec![ManifestItem {
                ident: vec![item.to_string()],
                kind: ItemKindTag::Function,
                hash: None,
                uuid: String::new(),
                span: (0, 1),
                summary: String::new(),
            }],
            prelink: None,
        }
    }

    fn manifest(modules: Vec<ManifestModule>) -> BuildManifest {
        BuildManifest {
            version: ambient_engine::disk_store::MANIFEST_VERSION,
            package_name: "test".to_string(),
            dispatch_surface_hash: [0u8; 32],
            natives_contract_hash: [0u8; 32],
            core_cache_key: [0u8; 32],
            modules,
        }
    }

    #[test]
    fn lists_live_package_symbols_excluding_builtins() {
        let dir = write_pkg(&[
            ("main.ab", "pub fn run(): Number { 1 }\n"),
            (
                "utils.ab",
                "pub fn helper(): Number { 2 }\npub struct Point { x: Number }\n",
            ),
        ]);
        let ifaces = interfaces(&dir);
        let syms = workspace_symbols("", &ifaces, None);

        let found = names(&syms);
        assert!(found.contains(&"run"), "run missing: {found:?}");
        assert!(found.contains(&"helper"), "helper missing: {found:?}");
        assert!(found.contains(&"Point"), "Point missing: {found:?}");

        // Builtins (core/platform) have no on-disk file and are excluded.
        assert!(
            syms.iter().all(|s| !s.source_path.is_empty()),
            "a builtin (empty source_path) leaked into workspace symbols"
        );
        assert!(
            syms.iter().all(|s| !s.module.starts_with("core")),
            "a core symbol leaked: {:?}",
            syms.iter().map(|s| &s.module).collect::<Vec<_>>()
        );
    }

    #[test]
    fn query_filters_case_insensitively() {
        let dir = write_pkg(&[("utils.ab", "pub fn helper(): Number { 2 }\n")]);
        let ifaces = interfaces(&dir);
        assert_eq!(
            names(&workspace_symbols("HEL", &ifaces, None)),
            vec!["helper"]
        );
        assert!(workspace_symbols("zzz", &ifaces, None).is_empty());
    }

    #[test]
    fn no_snapshot_serves_live_only() {
        let dir = write_pkg(&[("utils.ab", "pub fn helper(): Number { 2 }\n")]);
        let ifaces = interfaces(&dir);
        assert_eq!(names(&workspace_symbols("", &ifaces, None)), vec!["helper"]);
    }

    #[test]
    fn live_supersedes_snapshot_and_overlays_missing_modules() {
        let dir = write_pkg(&[("utils.ab", "pub fn helper(): Number { 2 }\n")]);
        let ifaces = interfaces(&dir);
        let utils_key = key_ending(&ifaces, "utils");

        // A snapshot that is stale for `utils` (records an old `oldHelper` that
        // the live buffer no longer has) and also carries a module the live set
        // does not cover at all (`ghost`).
        let snap = manifest(vec![
            manifest_module(&utils_key, "utils.ab", "oldHelper"),
            manifest_module("workspace::test::ghost", "ghost.ab", "phantom"),
        ]);

        let syms = workspace_symbols("", &ifaces, Some(&snap));
        let found = names(&syms);
        // Live wins for `utils`: the current symbol shows, the stale one does not.
        assert!(found.contains(&"helper"), "live helper missing: {found:?}");
        assert!(
            !found.contains(&"oldHelper"),
            "stale snapshot symbol served for a live module: {found:?}"
        );
        // The snapshot still covers a module the live set lacks.
        assert!(
            found.contains(&"phantom"),
            "overlay symbol missing: {found:?}"
        );
    }

    #[test]
    fn snapshot_builtin_modules_excluded() {
        let dir = write_pkg(&[("utils.ab", "pub fn helper(): Number { 2 }\n")]);
        let ifaces = interfaces(&dir);
        // A snapshot-only module with an empty source_path (a builtin) is not
        // navigable and must not surface.
        let snap = manifest(vec![manifest_module("core::extra", "", "hidden")]);
        let syms = workspace_symbols("", &ifaces, Some(&snap));
        let found = names(&syms);
        assert!(
            !found.contains(&"hidden"),
            "builtin overlay leaked: {found:?}"
        );
    }

    #[test]
    fn directory_module_uses_real_main_path() {
        let dir = write_pkg(&[("collections/main.ab", "pub fn seed(): Number { 1 }\n")]);
        let ifaces = interfaces(&dir);
        let syms = workspace_symbols("seed", &ifaces, None);
        assert_eq!(syms.len(), 1, "expected one symbol: {syms:?}");
        // The real on-disk `<dir>/main.ab` path, not the reconstructed
        // `collections.ab`.
        assert_eq!(syms[0].source_path, "collections/main.ab");
    }
}
