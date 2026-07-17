//! `ambient store` — inspect and maintain a package's content-addressed store.
//!
//! A workspace store roots one snapshot pointer per package. Whole-store
//! subcommands (stats, deps, verify, gc) never care; snapshot-reading ones
//! (snapshot, list, show, tag, diff) follow one package's pointer, selected
//! by [`target_package`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use ambient_engine::bytecode::disassemble;
use ambient_engine::disk_store::{BuildManifest, DiskStore};
use ambient_engine::object::StoredObject;

use crate::cli::StoreCommand;

mod index;
mod query;

/// Locate a package's store directory from a path. Workspace members share
/// one store at the workspace root; a standalone package owns its own —
/// [`ambient_engine::build::store_root`] is the single authority.
fn find_store_root(path: &Path) -> Result<PathBuf> {
    let start = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let canonical = start
        .canonicalize()
        .with_context(|| format!("no such path: {}", start.display()))?;

    let root = ambient_engine::build::store_root(&canonical);
    if !root.join("ambient.toml").exists() {
        bail!(
            "no ambient.toml found at or above {} — run inside a package, or `ambient run` first to create a store",
            canonical.display()
        );
    }
    Ok(DiskStore::package_store_path(&root))
}

/// Resolve which package's snapshot pointer a snapshot-reading subcommand
/// follows. Manifest context decides exactly as `run`/`build` do (explicit
/// `--package`, else the discovered package/member, erroring at a
/// multi-member root); without any manifest context the store's own
/// pointers decide — a sole pointer is unambiguous, several need
/// `--package`.
fn target_package(store: &DiskStore, path: &Path, package: Option<&str>) -> Result<String> {
    if let Some(name) = super::resolve_target_package(path, package)? {
        return Ok(name);
    }
    let mut names: Vec<String> = store
        .snapshot_pointers()?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    match names.len() {
        0 => bail!("this store has no snapshots — run `ambient build` to record one"),
        1 => Ok(names.remove(0)),
        n => bail!(
            "this store holds snapshots for {n} packages; pick one with \
             --package <NAME> (packages: {})",
            names.join(", ")
        ),
    }
}

/// The manifest hash `package`'s snapshot pointer names. `Ok(None)` when
/// the store has no snapshot pointers at all; an error naming the packages
/// that do have one when `package` is not among them.
fn pointer_for(store: &DiskStore, package: &str) -> Result<Option<blake3::Hash>> {
    if let Some(hash) = store.snapshot_pointer_for(package)? {
        return Ok(Some(hash));
    }
    let names: Vec<String> = store
        .snapshot_pointers()?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    if names.is_empty() {
        return Ok(None);
    }
    bail!(
        "no snapshot for package `{package}` in this store \
         (packages with snapshots: {})",
        names.join(", ")
    )
}

/// The selected package's snapshot manifest. `Ok(None)` when the store has
/// no snapshots, or this pointer's manifest is corrupt (a soft miss, like
/// [`DiskStore::current_snapshot_for`]); an unknown package name is an
/// error via [`pointer_for`].
fn snapshot_manifest(store: &DiskStore, package: &str) -> Result<Option<BuildManifest>> {
    if pointer_for(store, package)?.is_none() {
        return Ok(None);
    }
    Ok(store.current_snapshot_for(package)?)
}

/// Resolve a user-supplied reference (name from the names index, or a hash
/// prefix) to a full hash.
fn resolve_ref(store: &DiskStore, reference: &str) -> Result<blake3::Hash> {
    let names = store.names()?;
    if let Some(hash) = names.get(reference) {
        return Ok(*hash);
    }

    // Names are stored fully qualified (`workspace::<pkg>::geometry::circle::area`).
    // Accept a `::`-qualified suffix (`area`, `circle::area`) when it names a
    // single binding — the same ergonomic the runtime entry lookup allows.
    let suffix = format!("::{reference}");
    let suffix_matches: Vec<blake3::Hash> = names
        .iter()
        .filter(|(name, _)| name.ends_with(&suffix))
        .map(|(_, hash)| *hash)
        .collect();
    match suffix_matches.len() {
        0 => {}
        1 => return Ok(suffix_matches[0]),
        n => bail!("name suffix `{reference}` is ambiguous ({n} matches)"),
    }

    // Hash prefix lookup (like git short hashes).
    if reference.len() >= 4 && reference.chars().all(|c| c.is_ascii_hexdigit()) {
        let matches: Vec<blake3::Hash> = store
            .all_hashes()?
            .into_iter()
            .filter(|h| h.to_hex().as_str().starts_with(&reference.to_lowercase()))
            .collect();
        match matches.len() {
            0 => {}
            1 => return Ok(matches[0]),
            n => bail!("hash prefix {reference} is ambiguous ({n} matches)"),
        }
    }

    bail!("`{reference}` is neither a known name nor a hash prefix in this store")
}

/// Short display form of a hash.
fn short(hash: &blake3::Hash) -> String {
    hash.to_hex().as_str()[..12].to_string()
}

/// Names index inverted: hash → names bound to it.
fn hash_to_names(names: &BTreeMap<String, blake3::Hash>) -> HashMap<blake3::Hash, Vec<String>> {
    let mut inverted: HashMap<blake3::Hash, Vec<String>> = HashMap::new();
    for (name, hash) in names {
        inverted.entry(*hash).or_default().push(name.clone());
    }
    inverted
}

/// Run an `ambient store` subcommand.
pub fn cmd_store(path: &Path, package: Option<&str>, command: &StoreCommand) -> Result<()> {
    let store = DiskStore::open(find_store_root(path)?)?;
    // Only snapshot-reading subcommands resolve a target package; the
    // whole-store ones must keep working at a multi-member workspace root
    // (where resolution would demand `--package`).
    match command {
        StoreCommand::Stats => stats(&store),
        StoreCommand::Deps { reference } => deps(&store, reference),
        StoreCommand::Verify => verify(&store),
        StoreCommand::Gc => gc(&store),
        StoreCommand::List { kinds } => {
            let package = target_package(&store, path, package)?;
            index::list(&store, &package, kinds.as_deref())
        }
        StoreCommand::Show { reference } => {
            let package = target_package(&store, path, package)?;
            show(&store, &package, reference)
        }
        StoreCommand::Snapshot => {
            let package = target_package(&store, path, package)?;
            snapshot(&store, &package)
        }
        StoreCommand::Tag { name, target } => {
            let package = target_package(&store, path, package)?;
            query::tag(&store, &package, name.as_deref(), target.as_deref())
        }
        StoreCommand::Diff { a, b, format } => {
            let package = target_package(&store, path, package)?;
            query::diff(&store, &package, a.as_deref(), b.as_deref(), *format)
        }
    }
}

fn stats(store: &DiskStore) -> Result<()> {
    let mut plain = 0usize;
    let mut groups = 0usize;
    let mut group_members = 0usize;
    let mut redirects = 0usize;
    let mut values = 0usize;
    let mut natives = 0usize;
    let mut bytes = 0u64;

    for hash in store.all_hashes()? {
        bytes += std::fs::metadata(store.object_path(&hash))
            .map(|m| m.len())
            .unwrap_or(0);
        match store.get_object(&hash)? {
            Some(StoredObject::Plain(_)) => plain += 1,
            Some(StoredObject::Group(members)) => {
                groups += 1;
                group_members += members.len();
            }
            Some(StoredObject::Redirect { .. }) => redirects += 1,
            Some(StoredObject::Value(_)) => values += 1,
            Some(StoredObject::Native { .. }) => natives += 1,
            None => {}
        }
    }

    let names = store.names()?;
    println!("store:            {}", store.root().display());
    println!("functions:        {}", plain + group_members);
    println!("  plain:          {plain}");
    println!("  in groups:      {group_members} (across {groups} recursive groups)");
    println!("redirect stubs:   {redirects}");
    println!("const values:     {values}");
    println!("native fns:       {natives}");
    println!("named bindings:   {}", names.len());
    println!("disk usage:       {bytes} bytes");
    Ok(())
}

fn show(store: &DiskStore, package: &str, reference: &str) -> Result<()> {
    // A type/trait/ability is not an object: resolve it through the
    // structured index and print its kind, identity, span, and shape.
    if index::try_show(store, package, reference)? {
        return Ok(());
    }

    let hash = resolve_ref(store, reference)?;
    let names = store.names()?;
    let inverted = hash_to_names(&names);

    // Debug-symbol line: where this object is defined in source, if the
    // structured index knows.
    if let Some(location) = index::value_location(store, package, &hash) {
        println!("defined in: {location}");
    }

    let Some(object) = store.get_object(&hash)? else {
        bail!("no object stored at {hash}");
    };

    // Follow redirects to their group for display, but remember what the
    // user asked for.
    let (display_hash, object) = match object {
        StoredObject::Redirect { group, index } => {
            println!(
                "{} = member {index} of group {}",
                short(&hash),
                short(&group)
            );
            let Some(group_object) = store.get_object(&group)? else {
                bail!("group {group} is missing");
            };
            (group, group_object)
        }
        other => (hash, other),
    };

    match &object {
        StoredObject::Plain(_) => println!("object {} (plain function)", short(&display_hash)),
        StoredObject::Group(members) => println!(
            "object {} (recursive group, {} members)",
            short(&display_hash),
            members.len()
        ),
        StoredObject::Value(_) => println!("object {} (const value)", short(&display_hash)),
        StoredObject::Native { uuid, param_count } => println!(
            "object {} (native fn, uuid {uuid}, {param_count} params)",
            short(&display_hash)
        ),
        StoredObject::Redirect { .. } => unreachable!("redirects resolved above"),
    }
    println!("encoded size: {} bytes", object.encode().len());

    for (func_hash, func) in object
        .materialize()
        .map_err(|e| anyhow::anyhow!("malformed object: {e}"))?
    {
        println!();
        let bound_names = inverted
            .get(&func_hash)
            .map(|ns| format!(" ({})", ns.join(", ")))
            .unwrap_or_default();
        println!("fn {}{bound_names}", short(&func_hash));
        println!(
            "  params: {}  locals: {}  bytecode: {} bytes  deps: {}",
            func.param_count,
            func.local_count,
            func.bytecode.len(),
            func.dependencies.len()
        );
        for dep in &func.dependencies {
            let dep_names = inverted
                .get(dep)
                .map(|ns| format!(" ({})", ns.join(", ")))
                .unwrap_or_default();
            println!("  dep: {}{dep_names}", short(dep));
        }
        println!();
        for line in disassemble(&func).lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

fn deps(store: &DiskStore, reference: &str) -> Result<()> {
    let root = resolve_ref(store, reference)?;
    let names = store.names()?;
    let inverted = hash_to_names(&names);

    fn walk(
        store: &DiskStore,
        hash: &blake3::Hash,
        inverted: &HashMap<blake3::Hash, Vec<String>>,
        depth: usize,
        seen: &mut std::collections::HashSet<blake3::Hash>,
    ) -> Result<()> {
        let label = inverted
            .get(hash)
            .map(|ns| format!(" ({})", ns.join(", ")))
            .unwrap_or_default();
        let marker = if seen.contains(hash) { " *" } else { "" };
        println!(
            "{:indent$}{}{label}{marker}",
            "",
            short(hash),
            indent = depth * 2
        );
        if !seen.insert(*hash) {
            return Ok(());
        }
        if let Some(func) = store.get_function(hash)? {
            for dep in &func.dependencies {
                walk(store, dep, inverted, depth + 1, seen)?;
            }
        }
        Ok(())
    }

    let mut seen = std::collections::HashSet::new();
    walk(store, &root, &inverted, 0, &mut seen)?;
    println!(
        "\n{} function(s) in closure (* = already shown)",
        seen.len()
    );
    Ok(())
}

fn verify(store: &DiskStore) -> Result<()> {
    let report = store.verify()?;
    println!("{} object(s) valid", report.valid);
    for (hash, error) in &report.corrupt {
        println!("CORRUPT {}: {error}", short(hash));
    }
    for hash in &report.dangling {
        println!("DANGLING reference: {}", short(hash));
    }
    if let Some(reason) = &report.dangling_snapshot {
        println!("DANGLING snapshot: {reason}");
    }
    for (name, reason) in &report.bad_tags {
        println!("DANGLING tag {name}: {reason}");
    }
    for hash in &report.dangling_prelink {
        println!("DANGLING prelink: {}", short(hash));
    }
    if report.is_clean() {
        println!("store is clean");
        Ok(())
    } else {
        let mut extras = Vec::new();
        if report.dangling_snapshot.is_some() {
            extras.push("a broken snapshot");
        }
        if !report.bad_tags.is_empty() {
            extras.push("dangling tag(s)");
        }
        if !report.dangling_prelink.is_empty() {
            extras.push("dangling prelink blob(s)");
        }
        let extras = if extras.is_empty() {
            String::new()
        } else {
            format!(", and {}", extras.join(" and "))
        };
        bail!(
            "store has {} corrupt object(s), {} dangling reference(s){extras}",
            report.corrupt.len(),
            report.dangling.len(),
        );
    }
}

fn gc(store: &DiskStore) -> Result<()> {
    let removed = store.gc(&[])?;
    println!("removed {removed} unreachable object(s)");
    Ok(())
}

/// Short display form of a raw 32-byte hash.
fn short_bytes(bytes: &[u8; 32]) -> String {
    blake3::Hash::from_bytes(*bytes).to_hex().as_str()[..12].to_string()
}

fn snapshot(store: &DiskStore, package: &str) -> Result<()> {
    let Some(hash) = pointer_for(store, package)? else {
        println!("(no snapshot — run `ambient build` to record one)");
        return Ok(());
    };
    // A workspace store roots one snapshot per package; name the others so
    // nothing looks lost.
    let pointers = store.snapshot_pointers()?;
    if pointers.len() > 1 {
        let others: Vec<&str> = pointers
            .iter()
            .filter(|(name, _)| name != package)
            .map(|(name, _)| name.as_str())
            .collect();
        println!(
            "(workspace store with {} snapshots; showing `{package}` — others: {})",
            pointers.len(),
            others.join(", ")
        );
    }
    let Some(manifest) = store.load_manifest(&hash)? else {
        // Pointer present but the manifest is missing/corrupt: report it
        // rather than silently claim "no snapshot" (which `verify` also flags).
        bail!(
            "snapshot pointer names manifest {} but it is missing or corrupt (run `ambient store verify`)",
            short(&hash)
        );
    };

    println!("snapshot:         {}", short(&hash));
    println!("package:          {}", manifest.package_name);
    println!(
        "dispatch surface: {}",
        short_bytes(&manifest.dispatch_surface_hash)
    );
    println!(
        "natives contract: {}",
        short_bytes(&manifest.natives_contract_hash)
    );
    println!(
        "core cache key:   {}",
        short_bytes(&manifest.core_cache_key)
    );
    println!("modules:          {}", manifest.modules.len());
    println!();

    let width = manifest
        .modules
        .iter()
        .map(|m| m.module.len())
        .max()
        .unwrap_or(0);
    for module in &manifest.modules {
        // Builtin modules carry a zero per-module key (they validate through
        // the core unit key); show a dash rather than a run of zeros.
        let key = if module.cache_key == [0u8; 32] {
            "—".to_string()
        } else {
            short_bytes(&module.cache_key)
        };
        println!(
            "  {:<width$}  key {key}  links {}  objects {}",
            module.module,
            module.consumed_links.len(),
            module.objects.len(),
        );
    }
    Ok(())
}
