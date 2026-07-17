//! The structured-index views behind `ambient store list` and the type/trait/
//! ability half of `ambient store show`. These read the selected package's
//! current snapshot: its per-module item records (kind, identity, span,
//! source path) make items that are not objects — types, traits, abilities —
//! inspectable, and every listed name carries its namespace-tagged kind.

use anyhow::{Result, bail};

use ambient_engine::disk_store::{BuildManifest, DiskStore, ManifestItem};
use ambient_engine::module_interface::ItemKindTag;

use super::short_bytes;

/// One structured item paired with its enclosing module's identity and
/// source path.
pub struct IndexEntry<'a> {
    pub module: &'a str,
    pub source_path: &'a str,
    pub item: &'a ManifestItem,
}

impl IndexEntry<'_> {
    /// The item's fully-qualified name (`workspace::pkg::main::run`).
    fn fqn(&self) -> String {
        let mut out = String::from(self.module);
        for segment in &self.item.ident {
            out.push_str("::");
            out.push_str(segment);
        }
        out
    }

    /// The identity column: a short object hash for value items, a short
    /// uuid for nominal types/traits/abilities, else a dash.
    fn identity(&self) -> String {
        if let Some(hash) = &self.item.hash {
            short_bytes(hash)
        } else if !self.item.uuid.is_empty() {
            self.item.uuid.chars().take(12).collect()
        } else {
            "—".to_string()
        }
    }
}

/// Every structured item across a manifest, sorted by fully-qualified name.
fn all_entries(manifest: &BuildManifest) -> Vec<IndexEntry<'_>> {
    let mut entries: Vec<IndexEntry<'_>> = Vec::new();
    for module in &manifest.modules {
        for item in &module.items {
            entries.push(IndexEntry {
                module: &module.module,
                source_path: &module.source_path,
                item,
            });
        }
    }
    entries.sort_by_key(IndexEntry::fqn);
    entries
}

/// Whether a kind passes a comma-separated `--kinds` filter, matching either
/// the precise kind label (`struct`) or the namespace label (`type`).
fn kind_matches(kind: ItemKindTag, filter: &[String]) -> bool {
    filter.is_empty()
        || filter
            .iter()
            .any(|k| k == kind.label() || k == kind.namespace().label())
}

/// `ambient store list [--kinds ...]`.
pub fn list(store: &DiskStore, package: &str, kinds: Option<&str>) -> Result<()> {
    let filter: Vec<String> = kinds
        .map(|s| {
            s.split(',')
                .map(|k| k.trim().to_ascii_lowercase())
                .filter(|k| !k.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let Some(manifest) = super::snapshot_manifest(store, package)? else {
        return ls_flat_fallback(store, &filter);
    };

    let entries: Vec<IndexEntry<'_>> = all_entries(&manifest)
        .into_iter()
        .filter(|e| kind_matches(e.item.kind, &filter))
        .collect();

    // Every row: a kind label, an identity (short hash / short uuid), and the
    // full name. Structured items first (typed), then — only when unfiltered —
    // any name binding the index doesn't cover (content dispatch symbols,
    // lambdas), tagged with a bare kind so `ls` never *loses* a binding the
    // flat names file carries.
    let mut rows: Vec<(String, String, String)> = entries
        .iter()
        .map(|e| (e.item.kind.label().to_string(), e.identity(), e.fqn()))
        .collect();
    if filter.is_empty() {
        let indexed: std::collections::HashSet<String> = entries.iter().map(|e| e.fqn()).collect();
        for (name, hash) in store.names()? {
            if !indexed.contains(&name) {
                rows.push(("—".to_string(), short_bytes(hash.as_bytes()), name));
            }
        }
    }
    rows.sort_by(|a, b| a.2.cmp(&b.2));

    if rows.is_empty() {
        println!("(no matching items)");
        return Ok(());
    }
    let kind_width = rows.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
    let id_width = rows.iter().map(|(_, id, _)| id.len()).max().unwrap_or(0);
    for (kind, id, name) in &rows {
        println!("{kind:<kind_width$}  {id:<id_width$}  {name}");
    }
    Ok(())
}

/// The pre-snapshot fallback: list the flat names index (functions/consts
/// only, no kind data). A `--kinds` filter that excludes values yields
/// nothing here, since only value bindings exist in the flat index.
fn ls_flat_fallback(store: &DiskStore, filter: &[String]) -> Result<()> {
    if !filter.is_empty()
        && !filter
            .iter()
            .any(|k| matches!(k.as_str(), "value" | "fn" | "const"))
    {
        println!("(no snapshot — only value bindings are known; nothing matches --kinds)");
        return Ok(());
    }
    let names = store.names()?;
    if names.is_empty() {
        println!("(no named bindings — run `ambient run` to populate the store)");
        return Ok(());
    }
    let width = names.keys().map(String::len).max().unwrap_or(0);
    for (name, hash) in &names {
        println!("{} {name:<width$}", &hash.to_hex().as_str()[..12]);
    }
    Ok(())
}

/// Find the index entries a reference names: an exact fully-qualified match,
/// else a `::`-qualified suffix, else a bare final-name match.
pub fn find<'a>(manifest: &'a BuildManifest, reference: &str) -> Vec<IndexEntry<'a>> {
    let entries = all_entries(manifest);
    if let Some(exact) = entries.iter().position(|e| e.fqn() == reference) {
        return vec![clone_entry(&entries[exact])];
    }
    let suffix = format!("::{reference}");
    let by_suffix: Vec<IndexEntry<'a>> = entries
        .iter()
        .filter(|e| e.fqn().ends_with(&suffix))
        .map(clone_entry)
        .collect();
    if !by_suffix.is_empty() {
        return by_suffix;
    }
    entries
        .iter()
        .filter(|e| e.item.name() == reference)
        .map(clone_entry)
        .collect()
}

fn clone_entry<'a>(e: &IndexEntry<'a>) -> IndexEntry<'a> {
    IndexEntry {
        module: e.module,
        source_path: e.source_path,
        item: e.item,
    }
}

/// Try to resolve `reference` to a non-object item (type/trait/ability/alias)
/// and print it. Returns `true` if it handled the reference, `false` to fall
/// through to the object-disassembly path (a value item, or no index match).
pub fn try_show(store: &DiskStore, package: &str, reference: &str) -> Result<bool> {
    let Some(manifest) = super::snapshot_manifest(store, package)? else {
        return Ok(false);
    };
    let matches = find(&manifest, reference);
    // Only claim non-value items here; value items disassemble as objects.
    let non_values: Vec<&IndexEntry<'_>> =
        matches.iter().filter(|e| !e.item.kind.is_value()).collect();
    match non_values.len() {
        0 => Ok(false),
        1 => {
            show_item(non_values[0]);
            Ok(true)
        }
        n => bail!("`{reference}` is ambiguous ({n} matching types/traits/abilities)"),
    }
}

/// The source location of a value item bound to `hash`, if the selected
/// package's snapshot index knows it — the "defined at" line for
/// `store show` of a function/const.
pub fn value_location(store: &DiskStore, package: &str, hash: &blake3::Hash) -> Option<String> {
    let manifest = super::snapshot_manifest(store, package).ok().flatten()?;
    let bytes = hash.as_bytes();
    for module in &manifest.modules {
        for item in &module.items {
            if item.hash.as_ref() == Some(bytes) && !module.source_path.is_empty() {
                return Some(format!(
                    "{} [bytes {}..{}]",
                    module.source_path, item.span.0, item.span.1
                ));
            }
        }
    }
    None
}

fn show_item(entry: &IndexEntry<'_>) {
    println!("{} ({})", entry.fqn(), entry.item.kind.label());
    if !entry.item.uuid.is_empty() {
        println!("  uuid: {}", entry.item.uuid);
    }
    if entry.source_path.is_empty() {
        println!(
            "  defined in: <embedded> [bytes {}..{}]",
            entry.item.span.0, entry.item.span.1
        );
    } else {
        println!(
            "  defined in: {} [bytes {}..{}]",
            entry.source_path, entry.item.span.0, entry.item.span.1
        );
    }
    if !entry.item.summary.is_empty() {
        println!("  {}", entry.item.summary);
    }
}
