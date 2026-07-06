//! `FileSystem` ability - filesystem operations.
//!
//! Whole-file and directory operations at the abstraction level of
//! Node/Go/Python file APIs (no file descriptors or streaming). Backed
//! directly by `std::fs`; needs no external configuration.
//!
//! # API
//!
//! - `read(path: string) -> string` - Read file as UTF-8 text
//! - `write(path: string, content: string) -> ()` - Create/truncate a file
//! - `read_binary(path: string) -> Binary` - Read file as raw bytes
//! - `write_binary(path: string, data: Binary) -> ()` - Create/truncate a file
//! - `exists(path: string) -> bool` - Check whether a path exists
//! - `list(path: string) -> List<string>` - Sorted directory entry names
//! - `remove(path: string) -> ()` - Remove a file or empty directory
//! - `create_dir(path: string) -> ()` - Create a directory and missing parents
//!
//! # Errors
//!
//! Fallible operations raise **catchable exceptions** (via
//! [`VmError::exception`]), so Ambient code can recover with
//! `handle ... { Exception.throw(msg) => ... }`. `exists` is infallible and
//! returns `false` when the path can't be inspected. Argument-type mismatches
//! are programmer errors and remain fatal type errors.

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::require;

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
// Handlers
// ═══════════════════════════════════════════════════════════════════════════

/// `FileSystem.read(path: string) -> string` (UTF-8; invalid UTF-8 is an exception)
fn read(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.read: {e}")))?;
    Ok(Value::string(content))
}

/// `FileSystem.write(path: string, content: string) -> ()`
fn write(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    let content = arg_string(&ability.args, 1)?;
    std::fs::write(&path, content)
        .map_err(|e| VmError::exception(format!("FileSystem.write: {e}")))?;
    Ok(Value::Unit)
}

/// `FileSystem.read_binary(path: string) -> Binary`
fn read_binary(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    let data = std::fs::read(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.read_binary: {e}")))?;
    Ok(Value::binary(data))
}

/// `FileSystem.write_binary(path: string, data: Binary) -> ()`
fn write_binary(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    let data = arg_bytes(&ability.args, 1)?;
    std::fs::write(&path, data)
        .map_err(|e| VmError::exception(format!("FileSystem.write_binary: {e}")))?;
    Ok(Value::Unit)
}

/// `FileSystem.exists(path: string) -> bool` (infallible: false when uninspectable)
fn exists(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `FileSystem.list(path: string) -> List<string>` (sorted entry names)
fn list(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
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

/// `FileSystem.remove(path: string) -> ()` (file first, then empty directory)
fn remove(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    std::fs::remove_file(&path)
        .or_else(|_| std::fs::remove_dir(&path))
        .map_err(|e| VmError::exception(format!("FileSystem.remove: {e}")))?;
    Ok(Value::Unit)
}

/// `FileSystem.create_dir(path: string) -> ()` (`mkdir -p`)
fn create_dir(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let path = arg_string(&ability.args, 0)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| VmError::exception(format!("FileSystem.create_dir: {e}")))?;
    Ok(Value::Unit)
}

/// Register the `FileSystem` ability handlers on a VM.
///
/// IO failures raise catchable exceptions.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_fs(vm: &mut Vm, ability: &AbilityInterface) {
    vm.register_host_handler(ability.id, require(ability, "read"), Box::new(read));
    vm.register_host_handler(ability.id, require(ability, "write"), Box::new(write));
    vm.register_host_handler(
        ability.id,
        require(ability, "read_binary"),
        Box::new(read_binary),
    );
    vm.register_host_handler(
        ability.id,
        require(ability, "write_binary"),
        Box::new(write_binary),
    );
    vm.register_host_handler(ability.id, require(ability, "exists"), Box::new(exists));
    vm.register_host_handler(ability.id, require(ability, "list"), Box::new(list));
    vm.register_host_handler(ability.id, require(ability, "remove"), Box::new(remove));
    vm.register_host_handler(
        ability.id,
        require(ability, "create_dir"),
        Box::new(create_dir),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_core::AbilityId;

    type Handler = fn(&SuspendedAbility) -> Result<Value, VmError>;

    /// Invoke a handler with the given args.
    fn call(handler: Handler, args: Vec<Value>) -> Result<Value, VmError> {
        handler(&SuspendedAbility {
            ability_id: AbilityId::from_bytes([5; 32]),
            method_id: 0,
            args,
        })
    }

    /// A unique temp path for this test run.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ambient_fs_unit_{}_{name}", std::process::id()))
    }

    #[test]
    fn test_fs_write_read_roundtrip() {
        let path = temp_path("roundtrip.txt");
        let path_str = path.to_string_lossy().into_owned();

        let result = call(
            write,
            vec![Value::string(&*path_str), Value::string("hello fs")],
        );
        assert_eq!(result.unwrap(), Value::Unit);

        let result = call(read, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::string("hello fs"));

        let result = call(exists, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(true));

        call(remove, vec![Value::string(&*path_str)]).unwrap();
        let result = call(exists, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_fs_read_missing_file_is_catchable_exception() {
        let path = temp_path("does_not_exist.txt");
        let result = call(read, vec![Value::string(path.to_string_lossy())]);
        match result {
            Err(VmError::Exception(_)) => {}
            other => panic!("expected catchable exception, got {other:?}"),
        }
    }

    #[test]
    fn test_fs_type_mismatch_is_type_error() {
        let result = call(read, vec![Value::Number(42.0)]);
        assert!(matches!(result, Err(VmError::TypeErrorOwned { .. })));
    }
}
