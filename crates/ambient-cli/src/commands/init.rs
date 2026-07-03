//! Init command implementation.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use ambient_engine::manifest::MANIFEST_FILENAME;

/// Default content for ambient.toml.
fn manifest_content(name: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"

[build]
src = "src"
"#
    )
}

/// Default content for src/main.ab.
const MAIN_CONTENT: &str = r#"pub fn run(): () with Console {
    platform::Console::print!("Hello, world!");
}
"#;

/// Initialize a new Ambient package.
pub fn cmd_init(path: &Path, name: Option<&str>) -> Result<()> {
    // Create directory if it doesn't exist
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create directory: {}", path.display()))?;
    }

    // Check that it's a directory
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }

    // Check that ambient.toml doesn't already exist
    let manifest_path = path.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        bail!("{} already exists in {}", MANIFEST_FILENAME, path.display());
    }

    // Determine package name
    let pkg_name = match name {
        Some(n) => n.to_string(),
        None => path
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_else(|| "my_package".to_string()),
    };

    // Validate package name
    if !is_valid_package_name(&pkg_name) {
        bail!(
            "invalid package name: '{}'. Use lowercase letters, numbers, and underscores.",
            pkg_name
        );
    }

    // Create src directory
    let src_dir = path.join("src");
    if !src_dir.exists() {
        fs::create_dir_all(&src_dir)
            .with_context(|| format!("failed to create src directory: {}", src_dir.display()))?;
    }

    // Write ambient.toml
    fs::write(&manifest_path, manifest_content(&pkg_name))
        .with_context(|| format!("failed to write {}", manifest_path.display()))?;

    // Write src/main.ab
    let main_path = src_dir.join("main.ab");
    if !main_path.exists() {
        fs::write(&main_path, MAIN_CONTENT)
            .with_context(|| format!("failed to write {}", main_path.display()))?;
    }

    // Ignore the package-local store (derived, content-addressed).
    let gitignore_path = path.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(&gitignore_path, ".ambient/\n")
            .with_context(|| format!("failed to write {}", gitignore_path.display()))?;
    }

    // Print success message
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    println!("Created {} in {}", MANIFEST_FILENAME, canonical.display());
    println!("Created src/main.ab");

    Ok(())
}

/// Check if a package name is valid.
fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // Must start with a letter
    let first = name.chars().next().unwrap_or('0');
    if !first.is_ascii_lowercase() {
        return false;
    }

    // Must contain only lowercase letters, numbers, and underscores
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_valid_package_names() {
        assert!(is_valid_package_name("my_package"));
        assert!(is_valid_package_name("foo"));
        assert!(is_valid_package_name("foo123"));
        assert!(is_valid_package_name("my_cool_package_2"));
    }

    #[test]
    fn test_invalid_package_names() {
        assert!(!is_valid_package_name(""));
        assert!(!is_valid_package_name("123foo"));
        assert!(!is_valid_package_name("MyPackage"));
        assert!(!is_valid_package_name("my-package"));
        assert!(!is_valid_package_name("_private"));
    }

    #[test]
    fn test_init_creates_package() {
        let dir = tempdir().expect("create temp dir");
        let pkg_path = dir.path().join("test_pkg");

        cmd_init(&pkg_path, None).expect("init should succeed");

        // Check files were created
        assert!(pkg_path.join("ambient.toml").exists());
        assert!(pkg_path.join("src/main.ab").exists());

        // Check manifest content
        let manifest = fs::read_to_string(pkg_path.join("ambient.toml")).expect("read manifest");
        assert!(manifest.contains("name = \"test_pkg\""));
        assert!(manifest.contains("version = \"0.1.0\""));

        // Check main.ab content
        let main = fs::read_to_string(pkg_path.join("src/main.ab")).expect("read main");
        assert!(main.contains("pub fn run()"));
    }

    #[test]
    fn test_init_with_custom_name() {
        let dir = tempdir().expect("create temp dir");
        let pkg_path = dir.path().join("some_dir");

        cmd_init(&pkg_path, Some("custom_name")).expect("init should succeed");

        let manifest = fs::read_to_string(pkg_path.join("ambient.toml")).expect("read manifest");
        assert!(manifest.contains("name = \"custom_name\""));
    }

    #[test]
    fn test_init_fails_on_existing_manifest() {
        let dir = tempdir().expect("create temp dir");
        fs::write(dir.path().join("ambient.toml"), "existing").expect("write manifest");

        let result = cmd_init(dir.path(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_init_fails_on_invalid_name() {
        let dir = tempdir().expect("create temp dir");
        let pkg_path = dir.path().join("test");

        let result = cmd_init(&pkg_path, Some("Invalid-Name"));
        assert!(result.is_err());
    }
}
