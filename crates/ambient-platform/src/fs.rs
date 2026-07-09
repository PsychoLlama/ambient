//! `FileSystem` natives - filesystem operations.
//!
//! Whole-file and directory operations at the abstraction level of
//! Node/Go/Python file APIs (no file descriptors or streaming). Backed
//! directly by `std::fs`; needs no external configuration.
//!
//! # Errors
//!
//! Fallible operations raise **catchable exceptions** (via
//! [`VmError::exception`]), so Ambient code can recover with
//! `handle ... { Exception.throw(msg) => ... }`. `fs_exists` is infallible
//! and returns `false` when the path can't be inspected. Argument-type
//! mismatches are programmer errors and remain fatal type errors.

use std::sync::Arc;

use ambient_ability::{Value, VmError};
use ambient_engine::natives::NativeRegistry;

use crate::bind;

// ═══════════════════════════════════════════════════════════════════════════
// Argument extraction helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Extract a string argument by index; a mismatch is a programmer error.
fn arg_string(args: &[Value], index: usize) -> Result<String, VmError> {
    match args.get(index) {
        Some(Value::String(s)) => Ok(s.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "String".to_string(),
            got: other
                .map_or("missing argument", Value::type_name)
                .to_string(),
        }),
    }
}

/// Extract a bytes argument by index; a mismatch is a programmer error.
fn arg_bytes(args: &[Value], index: usize) -> Result<Vec<u8>, VmError> {
    match args.get(index) {
        Some(Value::Binary(b)) => Ok(b.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "Binary".to_string(),
            got: other
                .map_or("missing argument", Value::type_name)
                .to_string(),
        }),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Implementations
// ═══════════════════════════════════════════════════════════════════════════

/// `fs_read(path: string) -> string` (UTF-8; invalid UTF-8 is an exception)
fn read(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.read: {e}")))?;
    Ok(Value::string(content))
}

/// `fs_write(path: string, content: string) -> ()`
fn write(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    let content = arg_string(args, 1)?;
    std::fs::write(&path, content)
        .map_err(|e| VmError::exception(format!("FileSystem.write: {e}")))?;
    Ok(Value::Unit)
}

/// `fs_read_binary(path: string) -> Binary`
fn read_binary(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    let data = std::fs::read(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.read_binary: {e}")))?;
    Ok(Value::binary(data))
}

/// `fs_write_binary(path: string, data: Binary) -> ()`
fn write_binary(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    let data = arg_bytes(args, 1)?;
    std::fs::write(&path, data)
        .map_err(|e| VmError::exception(format!("FileSystem.write_binary: {e}")))?;
    Ok(Value::Unit)
}

/// `fs_exists(path: string) -> bool` (infallible: false when uninspectable)
fn exists(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `fs_list(path: string) -> List<string>` (sorted entry names)
fn list(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    let entries = std::fs::read_dir(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.list: {e}")))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| VmError::exception(format!("FileSystem.list: {e}")))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(Value::list(names.into_iter().map(Value::string).collect()))
}

/// `fs_remove(path: string) -> ()` (file first, then empty directory)
fn remove(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    std::fs::remove_file(&path)
        .or_else(|_| std::fs::remove_dir(&path))
        .map_err(|e| VmError::exception(format!("FileSystem.remove: {e}")))?;
    Ok(Value::Unit)
}

/// `fs_create_dir(path: string) -> ()` (`mkdir -p`)
fn create_dir(args: &[Value]) -> Result<Value, VmError> {
    let path = arg_string(args, 0)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.create_dir: {e}")))?;
    Ok(Value::Unit)
}

/// The `FileSystem` native implementations. IO failures raise catchable
/// exceptions.
#[must_use]
pub fn fs_natives() -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    bind(
        &mut registry,
        "fs_read",
        Arc::new(|args: Vec<Value>| read(&args)),
    );
    bind(
        &mut registry,
        "fs_write",
        Arc::new(|args: Vec<Value>| write(&args)),
    );
    bind(
        &mut registry,
        "fs_read_binary",
        Arc::new(|args: Vec<Value>| read_binary(&args)),
    );
    bind(
        &mut registry,
        "fs_write_binary",
        Arc::new(|args: Vec<Value>| write_binary(&args)),
    );
    bind(
        &mut registry,
        "fs_exists",
        Arc::new(|args: Vec<Value>| exists(&args)),
    );
    bind(
        &mut registry,
        "fs_list",
        Arc::new(|args: Vec<Value>| list(&args)),
    );
    bind(
        &mut registry,
        "fs_remove",
        Arc::new(|args: Vec<Value>| remove(&args)),
    );
    bind(
        &mut registry,
        "fs_create_dir",
        Arc::new(|args: Vec<Value>| create_dir(&args)),
    );
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp path for this test run.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ambient_fs_unit_{}_{name}", std::process::id()))
    }

    #[test]
    fn test_fs_write_read_roundtrip() {
        let path = temp_path("roundtrip.txt");
        let path_str = path.to_string_lossy().into_owned();

        let result = write(&[Value::string(&*path_str), Value::string("hello fs")]);
        assert_eq!(result.unwrap(), Value::Unit);

        let result = read(&[Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::string("hello fs"));

        let result = exists(&[Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(true));

        remove(&[Value::string(&*path_str)]).unwrap();
        let result = exists(&[Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_fs_read_missing_file_is_catchable_exception() {
        let path = temp_path("does_not_exist.txt");
        let result = read(&[Value::string(path.to_string_lossy())]);
        match result {
            Err(VmError::Exception(_)) => {}
            other => panic!("expected catchable exception, got {other:?}"),
        }
    }

    #[test]
    fn test_fs_type_mismatch_is_type_error() {
        let result = read(&[Value::Number(42.0)]);
        assert!(matches!(result, Err(VmError::TypeErrorOwned { .. })));
    }
}
