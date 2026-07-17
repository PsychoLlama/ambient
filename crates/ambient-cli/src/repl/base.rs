//! Session base state: project discovery, the base build every turn links
//! against, and the virtual session module's identity. Split from
//! `session.rs`, which owns the per-turn state these establish.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use ambient_analysis::package::AnalysisPackage;
use ambient_engine::build::BuildOptions;
use ambient_engine::compiler::CompiledModule;
use ambient_engine::fqn::NameKey;
use ambient_engine::module_path::ModulePath;

use super::session::SESSION_MODULE;
use crate::commands::core_context;
use crate::diagnostic::report_build_error;

/// Build the base compiled module and its analysis package. The optional
/// fourth element is a control note the frontend should surface (a
/// workspace base that degraded to member-only).
///
/// With a project, the base is the full package build (core + project, names
/// qualified) and the package is opened from disk. Without one, the base is
/// just the core library and the package is an empty in-memory shell.
pub(super) fn build_base(
    project_root: Option<&Path>,
) -> Result<(
    AnalysisPackage,
    CompiledModule,
    HashMap<NameKey, blake3::Hash>,
    Option<String>,
)> {
    match project_root {
        Some(root) => {
            // A workspace member's base builds at the *workspace* root so
            // every sibling compiles into the runtime base — an inline
            // `::lib::…` reference is then callable, not merely typeable.
            // The analysis package still opens at the member, so
            // `pkg`/`self`/`super` keep anchoring at the launch package
            // (mounted identities are the same either way).
            let build_root =
                match ambient_engine::workspace::Workspace::discover(&root.join("ambient.toml")) {
                    Ok(
                        ambient_engine::workspace::Discovered::Member(ws, _)
                        | ambient_engine::workspace::Discovered::WorkspaceRoot(ws),
                    ) => ws.root,
                    _ => root.to_path_buf(),
                };
            // A workspace build fails if *any* member fails, and the launch
            // member may itself be fine — degrade to a member-only base
            // (siblings stay typeable but not callable) instead of refusing
            // to start.
            let (result, note) = match build_at(&build_root) {
                Ok(result) => (result, None),
                Err(_) if build_root != root => {
                    let result = build_at(root).map_err(report_build_error)?;
                    (
                        result,
                        Some(
                            "note: a sibling workspace package fails to build; \
                             the base covers this package only, so `::pkg` \
                             references are not callable this session"
                                .to_string(),
                        ),
                    )
                }
                Err(err) => return Err(report_build_error(err)),
            };
            let package = AnalysisPackage::open(root).map_err(|e| anyhow!(e))?;
            Ok((package, result.compiled, result.link_table, note))
        }
        None => {
            let core = core_context()?;
            let package = AnalysisPackage::empty(PathBuf::from("."), PathBuf::from("."), "");
            Ok((package, core.compiled, core.hashes, None))
        }
    }
}

/// One base build rooted at `root`, warming off (never writing) the store
/// there.
fn build_at(
    root: &Path,
) -> Result<ambient_engine::build::BuildResult, ambient_engine::build::BuildError> {
    let stubs = ambient_platform::stub_natives();
    ambient_engine::build::build_package(
        root,
        crate::commands::parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            progress: None,
            // Warm the base build off a prior `ambient run`/`build`
            // snapshot: REPL startup on a built project skips recompiling
            // unchanged modules. The REPL is a read-only cache *consumer* —
            // it never writes a snapshot. A REPL session is not a canonical
            // build (its per-turn trial compiles are ephemeral), so
            // persisting one would only churn the store the real build
            // owns.
            store_path: Some(ambient_engine::disk_store::DiskStore::package_store_path(
                root,
            )),
            ..Default::default()
        },
    )
}

/// The virtual session module's path: `<launch_dir>/__repl.ab` mapped
/// through the package's file↔module convention, so `pkg`/`self`/`super`
/// resolve exactly as they would in a file authored where the REPL was
/// started. A launch directory outside the package's source tree (the
/// project root, or no project at all) anchors at the source root.
pub(super) fn session_module_path(package: &AnalysisPackage, launch_dir: &Path) -> ModulePath {
    let virtual_file = ambient_analysis::package::lexically_normalize(
        &launch_dir.join(format!("{SESSION_MODULE}.ab")),
    );
    package
        .module_path_for(&virtual_file)
        .or_else(|| {
            // Outside the source tree (or with no project): anchor at the
            // source root. Module paths are mounted under the package name,
            // so the fallback must be too — a bare path in a mounted
            // registry would resolve `pkg::`/`self` at the workspace root.
            if package.package_name().is_empty() {
                ModulePath::from_str_segments(&[SESSION_MODULE])
            } else {
                ModulePath::from_str_segments(&[package.package_name(), SESSION_MODULE])
            }
        })
        .expect("the reserved session module name is a valid module path")
}

/// Walk up from `dir` looking for an `ambient.toml` that marks a project root.
pub(super) fn find_project_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir;
    loop {
        if current.join("ambient.toml").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
