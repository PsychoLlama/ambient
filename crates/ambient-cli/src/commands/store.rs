//! `ambient store` — inspect and maintain a package's content-addressed store.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use ambient_engine::bytecode::disassemble;
use ambient_engine::disk_store::DiskStore;
use ambient_engine::object::StoredObject;

use crate::cli::StoreCommand;

/// Locate a package's store directory from a path (package root or any
/// directory containing `ambient.toml` upward).
fn find_store_root(path: &Path) -> Result<PathBuf> {
    let start = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let canonical = start
        .canonicalize()
        .with_context(|| format!("no such path: {}", start.display()))?;

    let mut dir: &Path = &canonical;
    loop {
        if dir.join("ambient.toml").exists() {
            return Ok(dir.join(".ambient").join("store"));
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => bail!(
                "no ambient.toml found at or above {} — run inside a package, or `ambient run` first to create a store",
                canonical.display()
            ),
        }
    }
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
pub fn cmd_store(path: &Path, command: &StoreCommand) -> Result<()> {
    let store = DiskStore::open(find_store_root(path)?)?;
    match command {
        StoreCommand::Stats => stats(&store),
        StoreCommand::Ls => ls(&store),
        StoreCommand::Show { reference } => show(&store, reference),
        StoreCommand::Deps { reference } => deps(&store, reference),
        StoreCommand::Verify => verify(&store),
        StoreCommand::Gc => gc(&store),
    }
}

fn stats(store: &DiskStore) -> Result<()> {
    let mut plain = 0usize;
    let mut groups = 0usize;
    let mut group_members = 0usize;
    let mut redirects = 0usize;
    let mut values = 0usize;
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
    println!("named bindings:   {}", names.len());
    println!("disk usage:       {bytes} bytes");
    Ok(())
}

fn ls(store: &DiskStore) -> Result<()> {
    let names = store.names()?;
    if names.is_empty() {
        println!("(no named bindings — run `ambient run` to populate the store)");
        return Ok(());
    }
    let width = names.keys().map(String::len).max().unwrap_or(0);
    for (name, hash) in &names {
        println!("{} {name:<width$}", short(hash));
    }
    Ok(())
}

fn show(store: &DiskStore, reference: &str) -> Result<()> {
    let hash = resolve_ref(store, reference)?;
    let names = store.names()?;
    let inverted = hash_to_names(&names);

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
    if report.is_clean() {
        println!("store is clean");
        Ok(())
    } else {
        bail!(
            "store has {} corrupt object(s) and {} dangling reference(s)",
            report.corrupt.len(),
            report.dangling.len()
        );
    }
}

fn gc(store: &DiskStore) -> Result<()> {
    let removed = store.gc(&[])?;
    println!("removed {removed} unreachable object(s)");
    Ok(())
}
