//! Rendering for the session's registry browsing (`core::option`,
//! `pkg::utils::helper`): module listings and member signatures. *What is
//! accessible* is decided by the registry's canonical lookups in
//! [`session`](super::session); this module shapes those facts into
//! displayable [`Value`]s. Split from the session for the per-file line
//! budget.

use std::sync::Arc;

use ambient_engine::ast::{Item, ItemKind};
use ambient_engine::module_path::ModulePath;
use ambient_engine::module_registry::{ExportKind, ModuleInfo, ModuleRegistry};
use ambient_engine::value::{ModuleExport, ModuleExportKind, ModuleValue, Value};

/// Map a registry export kind to its value-level rendering kind.
pub(crate) fn export_kind(kind: ExportKind) -> ModuleExportKind {
    match kind {
        ExportKind::Function => ModuleExportKind::Function,
        ExportKind::Const => ModuleExportKind::Const,
        ExportKind::Struct | ExportKind::TypeAlias => ModuleExportKind::Type,
        ExportKind::Enum => ModuleExportKind::Enum,
        ExportKind::EnumVariant => ModuleExportKind::Variant,
        // Ability methods are never module-level exports; render one like
        // its ability if it ever appears here.
        ExportKind::Ability | ExportKind::Trait | ExportKind::AbilityMethod | ExportKind::Set => {
            ModuleExportKind::Ability
        }
    }
}

/// Build a browsable module listing for `segments` (empty = the package
/// root), or `None` if nothing lives there.
///
/// The listing is the module's *accessible surface*: its `pub` exports
/// (functions with their signatures), its re-exports, and its child
/// modules — a directory namespace has only children, the package root
/// lists the package's top-level modules. The session module itself never
/// lists.
pub(crate) fn module_listing(
    registry: &ModuleRegistry,
    segments: &[Arc<str>],
    session_path: &ModulePath,
    package_root: &[Arc<str>],
) -> Option<Value> {
    let info = ModulePath::from_segments(segments.to_vec()).and_then(|p| registry.get(&p));

    // Child modules: every registered module directly under this path. The
    // package root additionally filters to workspace modules (`core` and
    // the platform declarations are not package members).
    let mut children: Vec<Arc<str>> = registry
        .all_modules()
        .filter_map(|m| {
            let segs = m.path.segments();
            let is_child =
                segs.len() == segments.len() + 1 && segs[..segments.len()] == segments[..];
            let hidden =
                m.path == *session_path || (segments.is_empty() && segs[0].as_ref() == "core");
            (is_child && !hidden).then(|| Arc::clone(&segs[segs.len() - 1]))
        })
        .collect();
    children.sort();
    children.dedup();

    if info.is_none() && children.is_empty() {
        return None;
    }

    let mut exports: Vec<ModuleExport> = Vec::new();
    let mut seen_modules: std::collections::HashSet<Arc<str>> = std::collections::HashSet::new();
    if let Some(info) = info {
        for e in info.exports.values().filter(|e| e.is_public) {
            let mut export = ModuleExport::new(e.name.as_ref(), export_kind(e.kind));
            export.signature = export_signature(info, &e.name);
            exports.push(export);
        }
        // Re-exports show under their real kind: `pub use self::stdio::Stdio;`
        // lists as an ability, not a module. Only a whole-module re-export
        // (or one the lookup can't resolve) stays a module entry.
        for re in &info.re_exports {
            let Some(name) = re.exported_name() else {
                continue;
            };
            match registry.lookup_symbol(&info.path, name) {
                Ok((export, defining)) => {
                    let mut entry = ModuleExport::new(name, export_kind(export.kind));
                    entry.signature = registry
                        .get(&defining)
                        .and_then(|di| export_signature(di, &export.name));
                    exports.push(entry);
                }
                Err(_) => {
                    if seen_modules.insert(Arc::from(name)) {
                        exports.push(ModuleExport::new(name, ModuleExportKind::Module));
                    }
                }
            }
        }
    }
    for child in children {
        if seen_modules.insert(Arc::clone(&child)) {
            exports.push(ModuleExport::new(child.as_ref(), ModuleExportKind::Module));
        }
    }

    Some(Value::Module(Arc::new(ModuleValue::new(
        display_path(segments, package_root).as_str(),
        exports,
    ))))
}

/// The signature suffix a module listing shows for an export: the part of
/// the item's declaration after its name (`(x: Number): Number with Log`
/// for a function, the type for a const). `None` when there is nothing
/// useful to show.
fn export_signature(info: &ModuleInfo, name: &str) -> Option<Arc<str>> {
    let item =
        info.module.items.iter().find(|item| {
            ambient_analysis::queries::item_name(item).is_some_and(|n| &**n == name)
        })?;
    let sig = ambient_analysis::queries::item_signature(item);
    match &item.kind {
        ItemKind::Function(_) | ItemKind::ExternFn(_) => {
            sig.find('(').map(|i| Arc::from(&sig[i..]))
        }
        ItemKind::Const(_) => sig.find(": ").map(|i| Arc::from(&sig[i + ": ".len()..])),
        _ => None,
    }
}

/// The signature a member inspection shows. Starts from the shared
/// [`item_signature`](ambient_analysis::queries::item_signature) (the same
/// rendering LSP hover uses) and, for the kinds whose hover header stays
/// deliberately compact, appends the body a REPL explorer wants: an enum's
/// variants, an ability's or trait's method signatures.
pub(crate) fn inspect_signature(item: &Item) -> String {
    use ambient_analysis::queries::format_type;
    let mut sig = ambient_analysis::queries::item_signature(item);
    match &item.kind {
        ItemKind::Enum(e) => {
            sig.push_str(" {");
            for v in &e.variants {
                sig.push_str("\n  ");
                sig.push_str(&v.name);
                if let Some(payload) = &v.payload {
                    sig.push('(');
                    sig.push_str(&format_type(payload));
                    sig.push(')');
                }
                sig.push(',');
            }
            sig.push_str("\n}");
        }
        ItemKind::Ability(a) => {
            sig.push_str(" {");
            for m in &a.methods {
                sig.push_str("\n  fn ");
                sig.push_str(&m.name);
                sig.push('(');
                for (i, param) in m.params.iter().enumerate() {
                    if i > 0 {
                        sig.push_str(", ");
                    }
                    sig.push_str(&param.name);
                    if let Some(ty) = &param.ty {
                        sig.push_str(": ");
                        sig.push_str(&format_type(ty));
                    }
                }
                sig.push_str("): ");
                sig.push_str(&format_type(&m.ret_ty));
                sig.push(';');
            }
            sig.push_str("\n}");
        }
        // With associated types the shared header already rendered a
        // `{ … }` block; only open one here when it didn't.
        ItemKind::Trait(t) if t.assoc_types.is_empty() => {
            sig.push_str(" {");
            for m in &t.methods {
                sig.push_str("\n  fn ");
                sig.push_str(&m.name);
                sig.push('(');
                if m.has_self {
                    sig.push_str("self");
                }
                for (i, (name, ty)) in m.params.iter().enumerate() {
                    if i > 0 || m.has_self {
                        sig.push_str(", ");
                    }
                    sig.push_str(name);
                    sig.push_str(": ");
                    sig.push_str(&format_type(ty));
                }
                sig.push_str("): ");
                sig.push_str(&format_type(&m.ret_ty));
                sig.push(';');
            }
            sig.push_str("\n}");
        }
        _ => {}
    }
    sig
}

/// The friendly rendering of absolute registry segments: `core::…` shows
/// as-is, package modules show under the `pkg` root (`pkg::net::client`),
/// the empty path is the root itself.
pub(crate) fn display_path(segments: &[Arc<str>], package_root: &[Arc<str>]) -> String {
    if segments.first().is_some_and(|s| s.as_ref() == "core") {
        return segments.join("::");
    }
    // Render the session package's mount as the `pkg` root the user types
    // (`["probe", "util"]` → `pkg::util`). In a mounted session anything
    // else is a sibling package's mount, which spells workspace-rooted
    // (`::lib::util`) — the path the user can actually type.
    let rest = if !package_root.is_empty() && segments.starts_with(package_root) {
        &segments[package_root.len()..]
    } else if package_root.is_empty() {
        segments
    } else {
        return format!("::{}", segments.join("::"));
    };
    let mut out = String::from("pkg");
    for seg in rest {
        out.push_str("::");
        out.push_str(seg);
    }
    out
}

/// Whether `s` is shaped like a module/member path: one or more identifier
/// segments joined by `::` — optionally workspace-rooted by a leading `::`
/// (`::lib::util`) — nothing else. Ordinary expressions (with operators,
/// whitespace, calls, or a `.`) are not path-shaped and skip introspection
/// so they never trigger a registry build.
pub(crate) fn looks_like_path(s: &str) -> bool {
    let s = s.strip_prefix("::").unwrap_or(s);
    !s.is_empty()
        && s.split("::").all(|seg| {
            let mut chars = seg.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}
