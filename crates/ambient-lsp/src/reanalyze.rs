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

use lsp_server::{Message, Notification};
use lsp_types::notification::{Notification as _, PublishDiagnostics};
use lsp_types::{Diagnostic, PublishDiagnosticsParams, Uri};

use ambient_analysis::occurrences::collect_occurrences;
use ambient_engine::module_path::ModulePath;

use crate::server::{
    DocumentAnalysis, ModuleOccurrences, ServerState, collect_diagnostics, module_uri,
};
use crate::util::uri_to_path;

/// Diagnostics computed for one document, ready for the transport to publish.
///
/// The notification/diagnostics path is split compute-from-send: reanalysis
/// *returns* these (a pure function of [`ServerState`]) and only the transport
/// loop turns them into wire messages via [`diagnostics_message`]. The test
/// harness reads them straight back instead, with no channel.
pub(crate) struct DiagnosticsUpdate {
    pub(crate) uri: Uri,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) version: i32,
}

/// Render a [`DiagnosticsUpdate`] as a `textDocument/publishDiagnostics`
/// notification message for the connection to send.
pub(crate) fn diagnostics_message(update: DiagnosticsUpdate) -> anyhow::Result<Message> {
    let params = PublishDiagnosticsParams {
        uri: update.uri,
        diagnostics: update.diagnostics,
        version: Some(update.version),
    };
    let notification = Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        serde_json::to_value(params)?,
    );
    Ok(Message::Notification(notification))
}

/// Re-analyze after a document change.
///
/// Feeds the edited file into the session — which updates the registry and
/// invalidates only the affected modules' memo entries — then re-analyzes
/// every open document (unchanged modules replay their cached diagnostics) and
/// refreshes the whole-package occurrence index. A signature change in one file
/// still surfaces (or clears) type errors in files that import it, because the
/// session recomputes the importers' keys.
pub(crate) fn reanalyze_all(changed_uri: &Uri, state: &mut ServerState) -> Vec<DiagnosticsUpdate> {
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
    let mut updates = Vec::with_capacity(uris.len());
    for uri in &uris {
        if let Some(update) = reanalyze_document(uri, state) {
            updates.push(update);
        }
    }

    // Refresh the occurrence index against the updated registry so
    // find-references and rename never see stale results.
    rebuild_occurrence_index(state);
    updates
}

/// Re-analyze one open document and publish fresh diagnostics.
///
/// A package document routes through the session memo; a document with no
/// package (or one outside its `src/`) checks as a stand-alone package root
/// against the core+platform registry, exactly like `ambient check` on a bare
/// file.
pub(crate) fn reanalyze_document(uri: &Uri, state: &mut ServerState) -> Option<DiagnosticsUpdate> {
    let version = state.documents.get(uri).map(|doc| doc.version)?;

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

    // Re-borrow the document only to render spans; caching is disjoint from the
    // session borrow above.
    let update = state.documents.get(uri).map(|doc| DiagnosticsUpdate {
        uri: uri.clone(),
        diagnostics: collect_diagnostics(Some(doc), &analysis.result),
        version,
    });
    state.analyses.insert(uri.as_str().to_string(), analysis);
    update
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

/// Project the session's occurrence index into the LSP's render cache,
/// attaching each module's file URI.
///
/// The index itself lives in `ambient-analysis` (`AnalysisSession`), which
/// rebuilds it module-scoped: `Item` occurrence identities are span-free
/// `Fqn`s, so a body edit re-collects only the edited module and every other
/// module's references stay valid. This renderer therefore never re-walks or
/// re-resolves — it reads the session's already-built lists (cloning them, so
/// the query handlers can borrow the cache freely) and maps each module to its
/// file. Walks every package module (opened or not) so references and rename
/// reach files never opened in the editor; with no package, each open document
/// is indexed as a root via a direct collect.
pub(crate) fn rebuild_occurrence_index(state: &mut ServerState) {
    let mut index = Vec::new();

    if let Some(session) = state.session.as_ref() {
        let package = session.package();
        for module in package.modules.values() {
            let Some(uri) = module_uri(Some(package), &module.path) else {
                continue;
            };
            let Some(occurrences) = session.occurrences_for(&module.path) else {
                continue;
            };
            index.push(ModuleOccurrences {
                module_path: module.path.clone(),
                uri,
                occurrences: occurrences.to_vec(),
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
