//! Package manifest parsing for `ambient.toml`.
//!
//! The manifest defines package metadata and build configuration.
//!
//! # Example
//!
//! ```toml
//! [package]
//! name = "my_project"
//! version = "0.1.0"
//!
//! [build]
//! src = "src"
//!
//! [host]
//! abilities = ["ambient:native"]
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// The manifest filename.
pub const MANIFEST_FILENAME: &str = "ambient.toml";

/// Errors that can occur when loading a manifest.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The manifest file could not be read.
    #[error("failed to read manifest: {0}")]
    Io(#[from] std::io::Error),

    /// The manifest file could not be parsed.
    #[error("failed to parse manifest: {0}")]
    Parse(#[from] toml::de::Error),

    /// The manifest is missing required fields.
    #[error("manifest missing required field: {0}")]
    MissingField(&'static str),

    /// No manifest was found in the directory or its parents.
    #[error("no {MANIFEST_FILENAME} found in {0} or any parent directory")]
    NotFound(PathBuf),
}

/// The default host abilities if `[host]` section is missing.
pub const DEFAULT_HOST_ABILITIES: &[&str] = &["ambient:native"];

/// Package manifest from `ambient.toml`.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Source directory relative to manifest (default: "src").
    pub src_dir: PathBuf,
    /// Host abilities available to this package (default: `["ambient:native"]`).
    ///
    /// Use `[]` for pure computation with no host abilities.
    pub host_abilities: Vec<String>,
}

/// Raw TOML structure for deserialization.
#[derive(Debug, Deserialize)]
struct RawManifest {
    package: Option<PackageSection>,
    build: Option<BuildSection>,
    host: Option<HostSection>,
}

#[derive(Debug, Deserialize)]
struct PackageSection {
    name: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BuildSection {
    src: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HostSection {
    abilities: Option<Vec<String>>,
}

impl Manifest {
    /// Load a manifest from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed, or if
    /// required fields are missing.
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let contents = std::fs::read_to_string(path)?;
        Self::parse(&contents)
    }

    /// Parse a manifest from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns an error if the string cannot be parsed or if required
    /// fields are missing.
    pub fn parse(contents: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(contents)?;

        let package = raw.package.ok_or(ManifestError::MissingField("package"))?;

        let name = package
            .name
            .ok_or(ManifestError::MissingField("package.name"))?;

        let version = package
            .version
            .ok_or(ManifestError::MissingField("package.version"))?;

        let src_dir = raw
            .build
            .and_then(|b| b.src)
            .map_or_else(|| PathBuf::from("src"), PathBuf::from);

        // Host abilities: use specified list, or default to ["ambient:native"]
        let host_abilities = raw.host.and_then(|h| h.abilities).unwrap_or_else(|| {
            DEFAULT_HOST_ABILITIES
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        });

        Ok(Self {
            name,
            version,
            src_dir,
            host_abilities,
        })
    }

    /// Find and load the manifest for a given path.
    ///
    /// If `path` is a file, looks for a manifest in its parent directory.
    /// If `path` is a directory, looks for a manifest in that directory.
    /// Walks up parent directories until a manifest is found.
    ///
    /// Returns the manifest and the directory containing it (the package root).
    ///
    /// # Errors
    ///
    /// Returns an error if no manifest is found or if it cannot be loaded.
    pub fn find(path: &Path) -> Result<(Self, PathBuf), ManifestError> {
        let start_dir = if path.is_file() {
            path.parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        } else {
            path.to_path_buf()
        };

        let mut current = start_dir.clone();
        loop {
            let manifest_path = current.join(MANIFEST_FILENAME);
            if manifest_path.is_file() {
                let manifest = Self::from_file(&manifest_path)?;
                return Ok((manifest, current));
            }

            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => return Err(ManifestError::NotFound(start_dir)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml = r#"
[package]
name = "test_pkg"
version = "0.1.0"
"#;
        let manifest = Manifest::parse(toml).expect("should parse");
        assert_eq!(manifest.name, "test_pkg");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.src_dir, PathBuf::from("src"));
        // Default host abilities
        assert_eq!(manifest.host_abilities, vec!["ambient:native"]);
    }

    #[test]
    fn test_parse_full_manifest() {
        let toml = r#"
[package]
name = "my_project"
version = "1.2.3"

[build]
src = "lib"

[host]
abilities = ["ambient:native"]
"#;
        let manifest = Manifest::parse(toml).expect("should parse");
        assert_eq!(manifest.name, "my_project");
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.src_dir, PathBuf::from("lib"));
        assert_eq!(manifest.host_abilities, vec!["ambient:native"]);
    }

    #[test]
    fn test_host_abilities_empty() {
        let toml = r#"
[package]
name = "pure_pkg"
version = "0.1.0"

[host]
abilities = []
"#;
        let manifest = Manifest::parse(toml).expect("should parse");
        assert!(manifest.host_abilities.is_empty());
    }

    #[test]
    fn test_host_abilities_custom() {
        let toml = r#"
[package]
name = "custom_pkg"
version = "0.1.0"

[host]
abilities = ["ambient:native", "my-plugin:dom"]
"#;
        let manifest = Manifest::parse(toml).expect("should parse");
        assert_eq!(
            manifest.host_abilities,
            vec!["ambient:native", "my-plugin:dom"]
        );
    }

    #[test]
    fn test_missing_package_section() {
        let toml = r#"
[build]
src = "src"
"#;
        let err = Manifest::parse(toml).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField("package")));
    }

    #[test]
    fn test_missing_name() {
        let toml = r#"
[package]
version = "0.1.0"
"#;
        let err = Manifest::parse(toml).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField("package.name")));
    }

    #[test]
    fn test_missing_version() {
        let toml = r#"
[package]
name = "test"
"#;
        let err = Manifest::parse(toml).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::MissingField("package.version")
        ));
    }

    #[test]
    fn test_invalid_toml() {
        let toml = "this is not valid toml {{{";
        let err = Manifest::parse(toml).unwrap_err();
        assert!(matches!(err, ManifestError::Parse(_)));
    }
}
