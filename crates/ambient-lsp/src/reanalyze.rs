//! Per-edit reanalysis: feed the changed document into the incremental
//! [`AnalysisSession`](ambient_analysis::session::AnalysisSession), republish
//! every open document's diagnostics through its memo, and refresh the
//! occurrence index.
//!
//! The server is a renderer: every decision about *what is an error*, and all
//! of the incremental machinery (registry updates, the per-module check memo,
//! cycle recomputation), lives in `ambient-analysis`. This module only drives
//! it per keystroke and hands the results to the LSP transport.

use std::sync::Arc;

use lsp_server::Connection;
use lsp_types::Uri;

use ambient_analysis::occurrences::collect_occurrences;
use ambient_engine::module_path::ModulePath;

use crate::server::{
    DocumentAnalysis, ModuleOccurrences, ServerState, collect_diagnostics, module_uri,
    publish_diagnostics,
};
use crate::util::uri_to_path;

/// Re-analyze after a document change.
///
/// Feeds the edited file into the session — which updates the registry and
/// invalidates only the affected modules' memo entries — then re-analyzes
/// every open document (unchanged modules replay their cached diagnostics) and
/// refreshes the whole-package occurrence index. A signature change in one file
/// still surfaces (or clears) type errors in files that import it, because the
/// session recomputes the importers' keys.
pub(crate) fn reanalyze_all(
    changed_uri: &Uri,
    state: &mut ServerState,
    connection: &Connection,
) -> anyhow::Result<()> {
    // Hand the changed document's current text to the session. `edit_module`
    // decides incremental-vs-full registry update and memo invalidation.
    if let Some(file_path) = uri_to_path(changed_uri)
        && let Some(doc) = state.documents.get(changed_uri)
    {
        let text = doc.text.clone();
        if let Some(session) = state.session.as_mut()
            && let Some(module_path) = session.package().module_path_for(&file_path)
        {
            session.edit_module(&module_path, text);
        }
    }

    let uris: Vec<Uri> = state.documents.uris().cloned().collect();
    for uri in uris {
        reanalyze_document(&uri, state, connection)?;
    }

    // Refresh the occurrence index against the updated registry so
    // find-references and rename never see stale results.
    rebuild_occurrence_index(state);
    Ok(())
}

/// Re-analyze one open document and publish fresh diagnostics.
///
/// A package document routes through the session memo; a document with no
/// package (or one outside its `src/`) checks as a stand-alone package root
/// against the core+platform registry, exactly like `ambient check` on a bare
/// file.
pub(crate) fn reanalyze_document(
    uri: &Uri,
    state: &mut ServerState,
    connection: &Connection,
) -> anyhow::Result<()> {
    let Some(version) = state.documents.get(uri).map(|doc| doc.version) else {
        return Ok(());
    };

    // Package path: memoized analysis through the session. The session already
    // holds this document's current text (fed in by `reanalyze_all`), so its
    // result matches a fresh `analyze_with_registry` on `doc.text`.
    let analysis = state.session.as_mut().and_then(|session| {
        let file_path = uri_to_path(uri)?;
        let module_path = session.package().module_path_for(&file_path)?;
        let result = session.analyze_module(&module_path)?;
        let registry = Arc::clone(session.registry());
        Some(DocumentAnalysis {
            result,
            module_path,
            registry,
        })
    });

    let analysis = match analysis {
        Some(analysis) => analysis,
        None => standalone_analysis(state, uri),
    };

    // Re-borrow the document only to render spans; publishing and caching are
    // disjoint from the session borrow above.
    if let Some(doc) = state.documents.get(uri) {
        let diagnostics = collect_diagnostics(Some(doc), &analysis.result);
        publish_diagnostics(connection, uri.clone(), diagnostics, version)?;
    }
    state.analyses.insert(uri.as_str().to_string(), analysis);
    Ok(())
}

/// Analyze a document with no package context: a stand-alone package root
/// against the core+platform registry.
fn standalone_analysis(state: &ServerState, uri: &Uri) -> DocumentAnalysis {
    let text = state
        .documents
        .get(uri)
        .map(|doc| doc.text.clone())
        .unwrap_or_default();
    let module_path = ModulePath::root();
    let mut registry = ambient_analysis::core_platform_registry();
    let recovered = ambient_parser::parse_recovering(&text);
    registry.register(&module_path, Arc::new(recovered.module));
    let registry = Arc::new(registry);
    let result =
        ambient_analysis::analyze_with_registry(&text, Some(&module_path), Some(&registry));
    DocumentAnalysis {
        result,
        module_path,
        registry,
    }
}

/// Rebuild the whole-package occurrence index from the current parsed modules
/// and registry.
///
/// Rebuilt in full every edit (not scoped to the changed module) on purpose: an
/// `Item` occurrence's identity is `(module, definition name span)`, and a body
/// edit that shifts a definition's span changes the target every *other*
/// module's references to it must match. A partial rebuild would leave those
/// cross-module references pointing at a stale span, so references/rename would
/// silently miss them. The walk is a pure AST pass that resolves names through
/// the already-built registry, so a full rebuild is cheap. Walks every package
/// module (opened or not) so references and rename reach files never opened in
/// the editor; with no package, each open document is indexed as a root.
pub(crate) fn rebuild_occurrence_index(state: &mut ServerState) {
    let mut index = Vec::new();

    if let Some(session) = state.session.as_ref() {
        let package = session.package();
        let registry = session.registry();
        for module in package.modules.values() {
            let Some(uri) = module_uri(Some(package), &module.path) else {
                continue;
            };
            let occurrences = collect_occurrences(&module.ast, &module.path, registry);
            index.push(ModuleOccurrences {
                module_path: module.path.clone(),
                uri,
                occurrences,
            });
        }
    } else {
        for (uri_str, analysis) in &state.analyses {
            let Ok(uri) = uri_str.parse::<Uri>() else {
                continue;
            };
            let occurrences = collect_occurrences(
                &analysis.result.module,
                &analysis.module_path,
                &analysis.registry,
            );
            index.push(ModuleOccurrences {
                module_path: analysis.module_path.clone(),
                uri,
                occurrences,
            });
        }
    }

    state.occurrences = index;
}
