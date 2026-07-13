//! `ambient store tag` and `ambient store diff`: naming build snapshots and
//! comparing two of them.

use anyhow::{Context, Result, bail};

use ambient_engine::disk_store::{BuildManifest, DiskStore, SnapshotDiff};

use super::short;

/// Resolve a snapshot reference to a manifest hash: the literal `current`
/// (the snapshot pointer), a tag name, or a manifest-hash prefix (git-style).
fn resolve_manifest_ref(store: &DiskStore, reference: &str) -> Result<blake3::Hash> {
    if reference == "current" {
        return store
            .snapshot_pointer()?
            .context("no current snapshot — run `ambient run` first");
    }
    if let Some(hash) = store.read_tag(reference)? {
        return Ok(hash);
    }
    if reference.len() >= 4 && reference.chars().all(|c| c.is_ascii_hexdigit()) {
        let needle = reference.to_lowercase();
        let matches: Vec<blake3::Hash> = store
            .all_manifest_hashes()?
            .into_iter()
            .filter(|h| h.to_hex().as_str().starts_with(&needle))
            .collect();
        match matches.len() {
            0 => {}
            1 => return Ok(matches[0]),
            n => bail!("manifest-hash prefix `{reference}` is ambiguous ({n} matches)"),
        }
    }
    bail!("`{reference}` is neither a tag, `current`, nor a manifest-hash prefix in this store")
}

/// Load the manifest a reference resolves to.
fn load_ref(store: &DiskStore, reference: &str) -> Result<BuildManifest> {
    let hash = resolve_manifest_ref(store, reference)?;
    store
        .load_manifest(&hash)?
        .with_context(|| format!("manifest {} is missing or corrupt", short(&hash)))
}

/// `ambient store tag [NAME [TARGET]]`.
pub fn tag(store: &DiskStore, name: Option<&str>, target: Option<&str>) -> Result<()> {
    let Some(name) = name else {
        return list_tags(store);
    };
    // Resolve the target manifest (explicit ref, or the current snapshot).
    let hash = match target {
        Some(target) => resolve_manifest_ref(store, target)?,
        None => store
            .snapshot_pointer()?
            .context("no current snapshot to tag — run `ambient run` first")?,
    };
    // A tag must name a real manifest, or `verify` would later flag it.
    if store.load_manifest(&hash)?.is_none() {
        bail!(
            "manifest {} is missing or corrupt; refusing to tag it",
            short(&hash)
        );
    }
    store.write_tag(name, &hash)?;
    println!("tagged {name} -> {}", short(&hash));
    Ok(())
}

fn list_tags(store: &DiskStore) -> Result<()> {
    let tags = store.list_tags()?;
    if tags.is_empty() {
        println!("(no tags — `ambient store tag <name>` to create one)");
        return Ok(());
    }
    let width = tags.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, hash) in &tags {
        println!("{name:<width$}  {}", short(hash));
    }
    Ok(())
}

/// `ambient store diff [A B] [--json]`.
pub fn diff(store: &DiskStore, a: Option<&str>, b: Option<&str>, json: bool) -> Result<()> {
    let (Some(a), Some(b)) = (a, b) else {
        bail!(
            "`store diff` needs two refs (each a tag, manifest-hash prefix, or `current`); \
             there is no recorded previous snapshot to default to"
        );
    };
    let from = load_ref(store, a)?;
    let to = load_ref(store, b)?;
    let diff = store.snapshot_diff(&from, &to);

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        render_human(a, b, &diff);
    }
    Ok(())
}

fn render_human(a: &str, b: &str, diff: &SnapshotDiff) {
    println!("diff {a} -> {b}");
    if diff.from_package != diff.to_package {
        println!("package: {} -> {}", diff.from_package, diff.to_package);
    }
    if diff.is_empty() {
        println!("(snapshots are identical)");
        return;
    }

    println!();
    println!("modules:");
    for m in &diff.modules.added {
        println!("  + {m}");
    }
    for m in &diff.modules.removed {
        println!("  - {m}");
    }
    for change in &diff.modules.changed {
        let how = if change.interface_changed {
            "interface"
        } else {
            "body-only"
        };
        println!("  ~ {} ({how})", change.module);
    }
    if diff.modules.added.is_empty()
        && diff.modules.removed.is_empty()
        && diff.modules.changed.is_empty()
    {
        println!("  (none)");
    }

    println!();
    println!("items:");
    print_binding_group("added", '+', &diff.bindings.added);
    print_binding_group("removed", '-', &diff.bindings.removed);
    print_binding_group("rebound", '~', &diff.bindings.rebound);
    print_binding_group("retired", '!', &diff.bindings.retired);
    if diff.bindings.added.is_empty()
        && diff.bindings.removed.is_empty()
        && diff.bindings.rebound.is_empty()
        && diff.bindings.retired.is_empty()
    {
        println!("  (none)");
    }

    println!();
    println!(
        "objects: +{} ({} bytes), -{} ({} bytes)",
        diff.objects.added.len(),
        diff.objects.added_bytes,
        diff.objects.removed.len(),
        diff.objects.removed_bytes,
    );
}

fn print_binding_group(label: &str, marker: char, names: &[String]) {
    for name in names {
        println!("  {marker} {name}  [{label}]");
    }
}
