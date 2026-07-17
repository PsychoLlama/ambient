//! Package discovery and registry building.
//!
//! One implementation of "load an Ambient package for analysis", shared by
//! `ambient check` and the language server. Modules parse with error
//! recovery, so a file mid-edit still contributes its parseable items to
//! cross-module resolution instead of vanishing from the package.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use ambient_engine::ast::Module;
use ambient_engine::fqn::ModuleId;
use ambient_engine::manifest::Manifest;
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::ModuleRegistry;
use ambient_parser::ParseError;

use crate::AnalysisResult;

/// One package module's resolve-pass dependency set: the foreign modules its
/// references canonicalized into, plus its own [`ModuleId`] (so both the
/// canonical-identity form the cache key uses and the dotted-path form the
/// cycle graph uses are recoverable without re-resolving).
#[derive(Debug, Clone)]
pub struct ModuleDeps {
    /// The module's own canonical identity.
    pub module_id: ModuleId,
    /// The foreign modules it depends on (all scopes: core + package).
    pub deps: std::collections::BTreeSet<ModuleId>,
}

/// One loaded workspace member: a mount name plus the directories it loads
/// from. All members are the same shape; which one is *primary* is a
/// position ([`AnalysisPackage::members`]`[0]`), not a different type.
#[derive(Debug, Clone)]
pub struct AnalysisMember {
    /// The member's package name — its mount segment. Empty only for the
    /// primary of an in-memory session with no manifest.
    pub name: String,
    /// The member's root directory (where its ambient.toml is).
    pub root: PathBuf,
    /// The member's source directory.
    pub src_dir: PathBuf,
}

/// A package opened for analysis: manifest info plus every parsed module.
///
/// When the package is a workspace member, every *other* member loads too
/// (mounted under their own names) — the same everything-registers shape
/// `build_package` uses, so `ambient check` and the LSP resolve `::pkg`
/// references identically to a build.
#[derive(Debug)]
pub struct AnalysisPackage {
    /// Every loaded member, primary first — never empty. Members are
    /// uniform for loading and resolution; the primary is special only as
    /// an anchor: its name is the workspace scope every user item's `Fqn`
    /// is minted under (so it must match what
    /// `ambient_engine::build::build_package` uses — otherwise a consumer
    /// that links analysis output against a build (the REPL) sees
    /// mismatched identities), and its root is where the session loads
    /// snapshots from.
    pub members: Vec<AnalysisMember>,
    /// Parsed modules across every loaded member, keyed by (mounted)
    /// module path string.
    pub modules: HashMap<String, ParsedModule>,
}

/// A parsed module with its source and (possibly partial) AST.
#[derive(Debug, Clone)]
pub struct ParsedModule {
    /// The module path.
    pub path: ModulePath,
    /// Whether the module is a directory module (backed by a `main.ab`),
    /// which anchors `self`/`super` at its own path. A mounted package
    /// root is one by construction. Derived from the source path at
    /// insertion; registration must pass it through.
    pub is_dir_module: bool,
    /// The source code.
    pub source: String,
    /// The parsed AST — partial when the file has syntax errors.
    pub ast: Module,
    /// Parse errors, if any.
    pub parse_errors: Vec<ParseError>,
    /// The module's real on-disk source path, relative to `src_dir`
    /// (`collections/main.ab`), when loaded from disk. `None` for an
    /// in-memory module (the REPL's synthetic module) with no backing file.
    /// Threaded into the registry so navigation resolves a directory module
    /// to its actual `main.ab` rather than a reconstructed `<dir>.ab`.
    pub source_path: Option<String>,
}

impl AnalysisPackage {
    /// Discover the package containing `file` — and, when it is a
    /// workspace member, every sibling member — via the same
    /// [`Workspace::discover`](ambient_engine::workspace::Workspace)
    /// upward walk the build uses.
    ///
    /// A package under a workspace that does not list it (a build error)
    /// falls back to standalone analysis here: the editor should still
    /// analyze the file rather than go dark.
    #[must_use]
    pub fn discover(file: &Path) -> Option<Self> {
        use ambient_engine::workspace::{Discovered, Workspace};
        match Workspace::discover(file) {
            Ok(Discovered::Package(manifest, root)) => {
                Some(Self::from_manifest(manifest, &root, Vec::new()))
            }
            Ok(Discovered::Member(workspace, index)) => {
                let member = &workspace.members[index];
                let siblings = sibling_members(&workspace, Some(index));
                Some(Self::from_manifest(
                    member.manifest.clone(),
                    &member.root,
                    siblings,
                ))
            }
            // A bare workspace root: analyze the whole workspace with the
            // first member as primary (the choice only picks defaults; every
            // member's modules load either way).
            Ok(Discovered::WorkspaceRoot(workspace)) => Self::from_workspace_root(&workspace),
            Ok(Discovered::None) => None,
            // Discovery errors (unlisted member, malformed root manifest):
            // fall back to the nearest single package so analysis degrades
            // instead of disappearing.
            Err(_) => {
                let (manifest, root) = Manifest::find(file).ok()?;
                Some(Self::from_manifest(manifest, &root, Vec::new()))
            }
        }
    }

    /// An analysis view over a whole workspace, primary'd on its first
    /// member. `None` for an empty workspace.
    #[must_use]
    pub fn from_workspace_root(workspace: &ambient_engine::workspace::Workspace) -> Option<Self> {
        let first = workspace.members.first()?;
        let siblings = sibling_members(workspace, Some(0));
        Some(Self::from_manifest(
            first.manifest.clone(),
            &first.root,
            siblings,
        ))
    }

    /// Assemble a package (plus siblings) without loading modules.
    fn from_manifest(manifest: Manifest, root: &Path, siblings: Vec<AnalysisMember>) -> Self {
        let primary = AnalysisMember {
            name: manifest.name,
            root: lexically_normalize(root),
            src_dir: lexically_normalize(&root.join(&manifest.src_dir)),
        };
        let mut members = vec![primary];
        members.extend(siblings);
        Self {
            members,
            modules: HashMap::new(),
        }
    }

    /// Create an empty, in-memory package with no modules loaded from disk.
    ///
    /// Used by the REPL, which accumulates a single synthetic `repl` module
    /// in memory and re-checks it each turn. `root`/`src_dir` are notional —
    /// nothing is read from them — but `insert_module` and `build_registry`
    /// work exactly as they do for an on-disk package. `name` mounts the
    /// package (empty stays unmounted); it is fixed at construction because
    /// inserted modules key off the mount.
    #[must_use]
    pub fn empty(root: PathBuf, src_dir: PathBuf, name: &str) -> Self {
        Self {
            members: vec![AnalysisMember {
                name: name.to_string(),
                root,
                src_dir,
            }],
            modules: HashMap::new(),
        }
    }

    /// The primary member — the package analysis was opened *at*.
    #[must_use]
    fn primary(&self) -> &AnalysisMember {
        &self.members[0]
    }

    /// The primary package root directory (where its ambient.toml is).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.primary().root
    }

    /// The primary package's source directory.
    #[must_use]
    pub fn src_dir(&self) -> &Path {
        &self.primary().src_dir
    }

    /// The primary package name (`[package].name`) — the workspace scope.
    /// Empty for an in-memory session with no manifest.
    #[must_use]
    pub fn package_name(&self) -> &str {
        &self.primary().name
    }

    /// Open the package (or whole workspace) rooted at `root` — its
    /// ambient.toml may be a package manifest or a workspace root — loading
    /// sibling workspace members when there are any.
    pub fn open(root: &Path) -> Result<Self, String> {
        let manifest_path = root.join("ambient.toml");
        if !manifest_path.is_file() {
            return Err(format!("no ambient.toml at {}", root.display()));
        }
        let mut package = Self::discover(&manifest_path)
            .ok_or_else(|| format!("no package at {}", root.display()))?;
        package.load_modules();
        Ok(package)
    }

    /// The primary package's mount segment, when it has one: module paths
    /// carry the package name exactly like the engine's build pipeline, so
    /// `ambient check`/LSP and `ambient run` mint identical identities. An
    /// in-memory session with no manifest (empty `package_name`) stays bare.
    #[must_use]
    fn mount(&self) -> Option<&str> {
        let name = self.package_name();
        (!name.is_empty()).then_some(name)
    }

    /// Every mounted member as `(mount, src_dir)`, primary first. An
    /// unmounted in-memory package (empty primary name) yields nothing.
    fn mounts(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.members
            .iter()
            .filter(|m| !m.name.is_empty())
            .map(|m| (m.name.as_str(), m.src_dir.as_path()))
    }

    /// The module path for a source file inside any loaded member.
    #[must_use]
    pub fn module_path_for(&self, file: &Path) -> Option<ModulePath> {
        if self.mount().is_none() {
            let relative = file.strip_prefix(self.src_dir()).ok()?;
            return ModulePath::from_relative_file_path(relative);
        }
        for (mount, src_dir) in self.mounts() {
            if let Ok(relative) = file.strip_prefix(src_dir) {
                return ModulePath::from_relative_file_path(&Path::new(mount).join(relative));
            }
        }
        None
    }

    /// The source file for a module path inside any loaded member.
    ///
    /// Prefers the module's recorded real on-disk path (so a directory module
    /// resolves to its actual `<dir>/main.ab`); falls back to the canonical
    /// file↔module reconstruction for a module never loaded from disk.
    #[must_use]
    pub fn file_for_module(&self, path: &ModulePath) -> PathBuf {
        let owner_src = |path: &ModulePath| -> PathBuf {
            let first = path.segments().first().map(AsRef::as_ref);
            self.mounts()
                .find(|(mount, _)| Some(*mount) == first)
                .map_or_else(
                    || self.src_dir().to_path_buf(),
                    |(_, src)| src.to_path_buf(),
                )
        };
        if let Some(source_path) = self
            .modules
            .get(&path.to_string())
            .and_then(|m| m.source_path.as_ref())
        {
            return owner_src(path).join(source_path);
        }
        // Strip the mount before reconstructing: `["foo", "utils"]` is
        // `src/utils.ab`, and the mount itself is the root `main.ab`.
        let segments = path.segments();
        let first = segments.first().map(AsRef::as_ref);
        if let Some((_, src_dir)) = self.mounts().find(|(mount, _)| Some(*mount) == first) {
            let relative = match ModulePath::from_segments(segments[1..].to_vec()) {
                Some(inner) => inner.to_file_path(),
                None => PathBuf::from("main.ab"),
            };
            return src_dir.join(relative);
        }
        self.src_dir().join(path.to_file_path())
    }

    /// Discover and parse every `.ab` file under every loaded member's
    /// source directory.
    ///
    /// Files with syntax errors still register with their parseable items,
    /// so the rest of the package resolves imports against them.
    pub fn load_modules(&mut self) {
        // Members are uniform here: an unmounted primary is just the empty
        // mount, which the join below leaves bare.
        let members: Vec<(String, PathBuf)> = self
            .members
            .iter()
            .map(|m| (m.name.clone(), m.src_dir.clone()))
            .collect();
        for (mount, src_dir) in members {
            for file in discover_ab_files(&src_dir) {
                let Ok(relative) = file.strip_prefix(&src_dir) else {
                    continue;
                };
                let mounted = if mount.is_empty() {
                    relative.to_path_buf()
                } else {
                    Path::new(&mount).join(relative)
                };
                let Some(module_path) = ModulePath::from_relative_file_path(&mounted) else {
                    continue;
                };
                let Ok(source) = std::fs::read_to_string(&file) else {
                    continue;
                };
                // The real on-disk path (relative to the owning member's
                // `src/`) is the authority on a directory module's
                // `<dir>/main.ab` layout — record it so navigation never
                // has to reconstruct one from the module path.
                let source_path = Some(relative.to_string_lossy().replace('\\', "/"));
                self.insert_module_with_path(module_path, source, source_path);
            }
        }
    }

    /// Insert or replace a module from source (e.g. an in-editor buffer),
    /// preserving any previously recorded on-disk source path — an editor edit
    /// re-parses the buffer but the file it came from is unchanged.
    pub fn insert_module(&mut self, path: ModulePath, source: String) {
        let source_path = self
            .modules
            .get(&path.to_string())
            .and_then(|prev| prev.source_path.clone());
        self.insert_module_with_path(path, source, source_path);
    }

    /// Insert an in-memory module addressed by its `src`-relative file path
    /// (`utils.ab`, `collections/main.ab`), recording that path as the module's
    /// source path exactly as [`load_modules`](Self::load_modules) would from
    /// disk. Lets an in-memory package (the LSP test harness) mirror a
    /// discovered on-disk package with no filesystem — the recorded source path
    /// is what navigation resolves a directory module's `<dir>/main.ab` from.
    /// Returns the derived [`ModulePath`], or `None` if the path is not under
    /// `src`.
    pub fn insert_module_at_path(
        &mut self,
        src_relative_path: &str,
        source: String,
    ) -> Option<ModulePath> {
        let file = self.src_dir().join(src_relative_path);
        let module_path = self.module_path_for(&file)?;
        let source_path = Some(src_relative_path.replace('\\', "/"));
        self.insert_module_with_path(module_path.clone(), source, source_path);
        Some(module_path)
    }

    /// Insert or replace a module, recording its real on-disk source path.
    fn insert_module_with_path(
        &mut self,
        path: ModulePath,
        source: String,
        source_path: Option<String>,
    ) {
        // The directory-module flag falls out of the same file↔module
        // mapping the path came from; a module with no backing file (the
        // REPL's synthetic module) is file-like. The owning mount is the
        // path's own leading segment (source paths are member-relative).
        let owner_mount = path
            .segments()
            .first()
            .filter(|first| self.mounts().any(|(mount, _)| mount == first.as_ref()))
            .cloned();
        let is_dir_module = source_path
            .as_deref()
            .and_then(|sp| {
                let mounted = match &owner_mount {
                    Some(mount) => Path::new(mount.as_ref()).join(sp),
                    None => PathBuf::from(sp),
                };
                ModulePath::from_relative_file_path_with_kind(&mounted)
            })
            .is_some_and(|(_, is_dir)| is_dir);
        let recovered = ambient_parser::parse_recovering(&source);
        let parse_errors = recovered.errors;
        let mut ast = recovered.module;
        ast.name = path
            .segments()
            .last()
            .cloned()
            .unwrap_or_else(|| Arc::from("main"));
        self.modules.insert(
            path.to_string(),
            ParsedModule {
                path,
                is_dir_module,
                source,
                ast,
                parse_errors,
                source_path,
            },
        );
    }

    /// Build the module registry: core + platform declaration modules plus
    /// every package module. This is the same registry shape the engine's
    /// build pipeline checks against.
    ///
    /// Two passes, mirroring the engine build pipeline (`build.rs`): register
    /// the raw ASTs so resolution can see every module (imports may point
    /// anywhere in the package), then canonicalize each module and *replace*
    /// its raw AST with the resolved one. The replacement matters because
    /// cross-module signature hydration reads a foreign item's signature
    /// straight from the registry — if that signature still spells an ability
    /// bare (e.g. `with Stdio`), it gets re-resolved against the *importing*
    /// module's scope and spuriously fails whenever that module doesn't also
    /// import the ability. Registering the resolved AST (`with platform.Stdio`)
    /// keeps `ambient check`/LSP in step with `ambient run` on multi-module
    /// packages.
    #[must_use]
    pub fn build_registry(&self) -> ModuleRegistry {
        self.build_registry_with_deps().0
    }

    /// Build the registry *and* capture each package module's resolve-pass
    /// dependency set (as canonical [`ModuleId`]s), keyed by the module's
    /// canonical identity string.
    ///
    /// The incremental [`crate::session`] needs these edges for two things it
    /// must not re-derive by re-resolving the world per module: each module's
    /// cache-key dependency-interface fold, and the package's import-cycle
    /// graph. [`build_registry`] discards them.
    #[must_use]
    pub fn build_registry_with_deps(
        &self,
    ) -> (
        ModuleRegistry,
        std::collections::BTreeMap<String, ModuleDeps>,
    ) {
        let mut registry = crate::core_platform_registry();
        // Mount the package — module paths carry the package name and the
        // registry scopes them under it — exactly as `build_package` does,
        // so the resolve pass below stamps identities that match a compiled
        // build. This keeps `ambient check`/LSP identity-consistent with
        // `ambient run`, and lets the REPL link its session module against
        // a `build_package` base.
        if let Some(primary) = self.mount() {
            for (mount, _) in self.mounts() {
                registry.add_mount(mount);
            }
            registry.set_workspace_name(primary);
        }
        for module in self.modules.values() {
            registry.register_module(
                &module.path,
                Arc::new(module.ast.clone()),
                module.is_dir_module,
            );
            if let Some(source_path) = &module.source_path {
                registry.set_source_path(&module.path, source_path.clone());
            }
        }
        let mut deps = std::collections::BTreeMap::new();
        for module in self.modules.values() {
            let mut ast = module.ast.clone();
            let outcome =
                ambient_engine::resolve::resolve_module(&mut ast, &module.path, &registry);
            let id = registry.module_id(&module.path);
            deps.insert(
                id.to_string(),
                ModuleDeps {
                    module_id: id,
                    deps: outcome.deps,
                },
            );
            registry.register_module(&module.path, Arc::new(ast), module.is_dir_module);
        }
        (registry, deps)
    }

    /// The content-keyed interface of every module in the package, computed
    /// from the resolved registry — the same derivation `build_package` runs
    /// (the decision logic lives once in the engine's
    /// [`module_interface`](ambient_engine::module_interface)). Keyed by the
    /// module's canonical identity string. Phase 4 of incremental
    /// compilation will consume this to key an editor-side cache; today it is
    /// an additive accessor with no other consumer.
    #[must_use]
    pub fn module_interfaces(
        &self,
    ) -> std::collections::BTreeMap<String, ambient_engine::module_interface::ModuleInterfaceSummary>
    {
        let registry = self.build_registry();
        ambient_engine::module_interface::build_interfaces(&registry)
    }

    /// Analyze every module in the package against a shared registry.
    ///
    /// Returns results keyed by module path string, in no particular
    /// order. This is what `ambient check <package>` reports, and what the
    /// LSP reports across open files — one computation, two renderers.
    ///
    /// A single cold pass: it shares the check + batch import-cycle path with
    /// the incremental [`crate::session::AnalysisSession`] (so cold and warm
    /// analysis are byte-identical, and cold `ambient check` also drops the
    /// per-module O(modules²) cycle re-resolve). The session adds a per-module
    /// memo on top; this one-shot has nothing to reuse, so it skips it.
    #[must_use]
    pub fn analyze_all(&self) -> HashMap<String, AnalysisResult> {
        let (registry, deps) = self.build_registry_with_deps();
        let cycles = crate::session::cycles_for(&registry, &deps);
        self.modules
            .iter()
            .map(|(key, module)| {
                let mut result = crate::check_without_cycle(
                    &module.source,
                    Some(&module.path),
                    Some(&registry),
                    None,
                );
                let cycle_key = registry.module_key(&registry.module_id(&module.path));
                result.import_cycle = cycles.get(&cycle_key).map(|cycle| {
                    crate::Diagnostic::error(
                        ambient_engine::ast::Span::new(0, 0),
                        cycle.describe(),
                        None,
                    )
                });
                (key.clone(), result)
            })
            .collect()
    }
}

/// Every member of `workspace` except `skip`, as loadable siblings.
fn sibling_members(
    workspace: &ambient_engine::workspace::Workspace,
    skip: Option<usize>,
) -> Vec<AnalysisMember> {
    workspace
        .members
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        .map(|(_, member)| AnalysisMember {
            name: member.name.clone(),
            root: lexically_normalize(&member.root),
            src_dir: lexically_normalize(&member.root.join(&member.manifest.src_dir)),
        })
        .collect()
}

/// Lexically normalize a path: drop `.` components and resolve `..` against the
/// preceding directory, purely syntactically — no filesystem access, so no
/// symlink surprises and it works on paths that don't exist yet (unlike
/// `fs::canonicalize`).
///
/// This is the canonical-path policy for every path an [`AnalysisPackage`] will
/// mint into a `file://` URI. A manifest `[build] src = "./"` would otherwise
/// leave `src_dir` (and thus every server-minted URI) carrying a literal `/./`
/// segment that no editor-sent URI contains — silently breaking any raw-string
/// URI comparison. Path-structural comparisons (`strip_prefix`, `starts_with`)
/// already fold `.` away, so this only has to match them at the mint boundary.
#[must_use]
pub fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // Root's parent is root; drop the `..`.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                // Nothing to pop (empty or a leading `..` chain): keep it.
                _ => out.push(component.as_os_str()),
            },
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Recursively discover all `.ab` files under a directory, sorted for
/// deterministic ordering.
fn discover_ab_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_ab_files(dir, &mut files);
    files.sort();
    files
}

fn collect_ab_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ab_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "ab") {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_package() -> TempDir {
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path();

        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("write manifest");

        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        fs::write(
            src.join("main.ab"),
            "use pkg::utils::helper;\npub fn run(): Number { helper() }",
        )
        .expect("write main");
        fs::write(src.join("utils.ab"), "pub fn helper(): Number { 1 }").expect("write utils");

        dir
    }

    #[test]
    fn open_loads_all_modules() {
        let dir = create_test_package();
        let package = AnalysisPackage::open(dir.path()).expect("open package");
        assert_eq!(package.modules.len(), 2);
        assert!(
            package.modules.contains_key("test"),
            "root module keys as the mount"
        );
        assert!(package.modules.contains_key("test::utils"));
    }

    #[test]
    fn analyze_all_is_clean_for_valid_package() {
        let dir = create_test_package();
        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();
        for (path, result) in &results {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }

    #[test]
    fn broken_module_still_contributes_parseable_items() {
        let dir = create_test_package();
        // utils has one broken and one good function; main imports the
        // good one and must still resolve.
        fs::write(
            dir.path().join("src/utils.ab"),
            "fn broken(\n\npub fn helper(): Number { 1 }",
        )
        .expect("rewrite utils");

        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();

        let utils = &results["test::utils"];
        assert!(!utils.parse_errors.is_empty());

        let main = &results["test"];
        assert!(
            main.diagnostics().is_empty(),
            "main should resolve helper from the partial module: {:?}",
            main.diagnostics()
        );
    }

    #[test]
    fn multi_module_use_imported_ability_checks_clean() {
        // Regression: `ambient check`/LSP must match `ambient run` on a
        // multi-module package whose entry point imports a platform ability
        // via `use`. The bug was that cross-module signature hydration read
        // `run`'s foreign `with Stdio` signature raw (bare `Stdio`) and
        // re-resolved it against the *other* module's scope, which doesn't
        // import Stdio — a false "not in scope" error. The sibling here is
        // deliberately empty of abilities: adding it must not change whether
        // `main` resolves its own import.
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path();
        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
        )
        .expect("write manifest");
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src dir");
        fs::write(
            src.join("main.ab"),
            "use core::system::Stdio;\n\npub fn run(): () with Stdio {\n    Stdio::out!(\"hi\")\n}\n",
        )
        .expect("write main");
        fs::write(src.join("sibling.ab"), "pub fn noop(): Number { 1 }\n").expect("write sibling");

        let package = AnalysisPackage::open(root).expect("open package");
        let results = package.analyze_all();
        for (path, result) in &results {
            assert!(
                result.diagnostics().is_empty(),
                "unexpected diagnostics in {path}: {:?}",
                result.diagnostics()
            );
        }
    }

    #[test]
    fn root_layout_src_dot_yields_clean_paths() {
        // Regression: an example-style manifest declares `[build] src = "./"`
        // with `main.ab` at the package root. Before normalization, `src_dir`
        // became `<root>/./`, so every minted URI carried a literal `/./` that
        // no editor-sent URI contained — breaking find-references/rename/goto.
        let dir = TempDir::new().expect("create temp dir");
        let root = dir.path();
        fs::write(
            root.join("ambient.toml"),
            "[package]\nname = \"strings\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"./\"\n",
        )
        .expect("write manifest");
        fs::write(root.join("main.ab"), "pub fn run(): Number { 1 }\n").expect("write main");

        // `discover` (the LSP's entry point) must produce a normalized src_dir.
        let main = root.join("main.ab");
        let mut package = AnalysisPackage::discover(&main).expect("discover package");
        let normalized_root = lexically_normalize(root);
        assert_eq!(
            package.src_dir(),
            normalized_root,
            "src = \"./\" should normalize src_dir to the package root"
        );
        assert!(
            !package.src_dir().to_string_lossy().contains("/./"),
            "src_dir must not carry a `.` component: {:?}",
            package.src_dir()
        );

        // And the module→file reconstruction every minted URI flows through must
        // itself be clean.
        package.load_modules();
        let module_path = package
            .module_path_for(&main)
            .expect("main resolves to a module");
        let file = package.file_for_module(&module_path);
        assert!(
            !file.components().any(|c| c == Component::CurDir),
            "file_for_module must not carry a `.` component: {file:?}"
        );
        assert_eq!(file, normalized_root.join("main.ab"));
    }

    #[test]
    fn lexically_normalize_folds_dot_and_dotdot() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/./c")),
            PathBuf::from("/a/b/c")
        );
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(lexically_normalize(Path::new("/a/./")), PathBuf::from("/a"));
        // Idempotent on an already-clean absolute path.
        assert_eq!(
            lexically_normalize(Path::new("/a/b/c")),
            PathBuf::from("/a/b/c")
        );
    }

    #[test]
    fn missing_import_is_reported() {
        let dir = create_test_package();
        fs::write(
            dir.path().join("src/main.ab"),
            "use pkg::utils::nonexistent;\npub fn run(): Number { 1 }",
        )
        .expect("rewrite main");

        let package = AnalysisPackage::open(dir.path()).expect("open package");
        let results = package.analyze_all();
        assert!(!results["test"].diagnostics().is_empty());
    }
}
