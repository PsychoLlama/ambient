//! Fs ability - filesystem operations.
//!
//! Whole-file and directory operations at the abstraction level of
//! Node/Go/Python file APIs (no file descriptors or streaming).
//!
//! # API
//!
//! - `read(path: string) -> string` - Read file as UTF-8 text
//! - `write(path: string, content: string) -> ()` - Create/truncate a file
//! - `read_bytes(path: string) -> Bytes` - Read file as raw bytes
//! - `write_bytes(path: string, data: Bytes) -> ()` - Create/truncate a file
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

use std::sync::OnceLock;

use ambient_ability::{HostHandler, RuntimeAbility, SuspendedAbility, Value, VmError};
use ambient_core::{
    hash_interface, AbilityDescriptor, AbilityId, MethodDescriptor, MethodId, MethodSignature,
    TypeFactory,
};

/// Method: read a file as UTF-8 text.
pub const METHOD_READ: u16 = 0x0000;

/// Method: write (create/truncate) a file with UTF-8 text.
pub const METHOD_WRITE: u16 = 0x0001;

/// Method: read a file as raw bytes.
pub const METHOD_READ_BYTES: u16 = 0x0002;

/// Method: write (create/truncate) a file with raw bytes.
pub const METHOD_WRITE_BYTES: u16 = 0x0003;

/// Method: check whether a path exists.
pub const METHOD_EXISTS: u16 = 0x0004;

/// Method: list directory entry names (sorted).
pub const METHOD_LIST: u16 = 0x0005;

/// Method: remove a file or empty directory.
pub const METHOD_REMOVE: u16 = 0x0006;

/// Method: create a directory and any missing parents.
pub const METHOD_CREATE_DIR: u16 = 0x0007;

/// The Fs ability's method set, instantiated for any type system.
///
/// Single source of truth for the interface: the content-addressed
/// [`ability_id`] and the engine-facing descriptor both derive from it.
fn methods<T: Clone + 'static>() -> Vec<MethodDescriptor<T>> {
    vec![
        MethodDescriptor {
            id: METHOD_READ,
            name: "read",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.string(),
            },
        },
        MethodDescriptor {
            id: METHOD_WRITE,
            name: "write",
            signature: MethodSignature {
                param_count: 2,
                param_types: |f| vec![f.string(), f.string()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_READ_BYTES,
            name: "read_bytes",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.bytes(),
            },
        },
        MethodDescriptor {
            id: METHOD_WRITE_BYTES,
            name: "write_bytes",
            signature: MethodSignature {
                param_count: 2,
                param_types: |f| vec![f.string(), f.bytes()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_EXISTS,
            name: "exists",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.bool(),
            },
        },
        MethodDescriptor {
            id: METHOD_LIST,
            name: "list",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.list(f.string()),
            },
        },
        MethodDescriptor {
            id: METHOD_REMOVE,
            name: "remove",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
        MethodDescriptor {
            id: METHOD_CREATE_DIR,
            name: "create_dir",
            signature: MethodSignature {
                param_count: 1,
                param_types: |f| vec![f.string()],
                return_type: |f| f.unit(),
            },
        },
    ]
}

/// The content-addressed identity of the Fs ability.
#[must_use]
pub fn ability_id() -> AbilityId {
    static ID: OnceLock<AbilityId> = OnceLock::new();
    *ID.get_or_init(|| hash_interface(FsAbility::NAME, &methods()))
}

/// Fs ability marker.
pub const FS: FsAbility = FsAbility;

/// Marker type for the Fs ability.
#[derive(Clone, Copy)]
pub struct FsAbility;

impl FsAbility {
    /// Ability name.
    pub const NAME: &'static str = "Fs";

    /// The content-addressed identity of the Fs ability.
    #[must_use]
    pub fn ability_id() -> AbilityId {
        ability_id()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Argument extraction helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Extract a string argument by index; a mismatch is a programmer error.
fn arg_string(args: &[Value], index: usize) -> Result<String, VmError> {
    match args.get(index) {
        Some(Value::String(s)) => Ok(s.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "string".to_string(),
            got: other
                .map_or("missing argument", Value::type_name)
                .to_string(),
        }),
    }
}

/// Extract a bytes argument by index; a mismatch is a programmer error.
fn arg_bytes(args: &[Value], index: usize) -> Result<Vec<u8>, VmError> {
    match args.get(index) {
        Some(Value::Bytes(b)) => Ok(b.as_ref().clone()),
        other => Err(VmError::TypeErrorOwned {
            expected: "bytes".to_string(),
            got: other
                .map_or("missing argument", Value::type_name)
                .to_string(),
        }),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Fs RuntimeAbility Implementation
// ═══════════════════════════════════════════════════════════════════════════

/// Fs ability implementation combining type info and handlers.
///
/// Backed directly by `std::fs`; needs no external configuration. IO
/// failures raise catchable exceptions.
#[derive(Default)]
pub struct FsRuntimeAbility;

impl FsRuntimeAbility {
    /// Create a new Fs ability.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl RuntimeAbility for FsRuntimeAbility {
    fn name(&self) -> &'static str {
        FsAbility::NAME
    }

    fn ability_id(&self) -> AbilityId {
        ability_id()
    }

    fn descriptor<T: Clone + 'static>(
        &self,
        _factory: &dyn TypeFactory<T>,
    ) -> AbilityDescriptor<T> {
        AbilityDescriptor {
            id: ability_id(),
            name: FsAbility::NAME,
            methods: Box::leak(methods::<T>().into_boxed_slice()),
        }
    }

    fn handlers(&self) -> Vec<(MethodId, HostHandler)> {
        // Fs.read(path: string) -> string (UTF-8; invalid UTF-8 is an exception)
        let read = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            let content = std::fs::read_to_string(&path)
                .map_err(|e| VmError::exception(format!("Fs.read: {e}")))?;
            Ok(Value::string(content))
        }) as HostHandler;

        // Fs.write(path: string, content: string) -> ()
        let write = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            let content = arg_string(&ability.args, 1)?;
            std::fs::write(&path, content)
                .map_err(|e| VmError::exception(format!("Fs.write: {e}")))?;
            Ok(Value::Unit)
        }) as HostHandler;

        // Fs.read_bytes(path: string) -> Bytes
        let read_bytes = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            let data = std::fs::read(&path)
                .map_err(|e| VmError::exception(format!("Fs.read_bytes: {e}")))?;
            Ok(Value::bytes(data))
        }) as HostHandler;

        // Fs.write_bytes(path: string, data: Bytes) -> ()
        let write_bytes = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            let data = arg_bytes(&ability.args, 1)?;
            std::fs::write(&path, data)
                .map_err(|e| VmError::exception(format!("Fs.write_bytes: {e}")))?;
            Ok(Value::Unit)
        }) as HostHandler;

        // Fs.exists(path: string) -> bool (infallible: false when uninspectable)
        let exists = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            Ok(Value::Bool(std::path::Path::new(&path).exists()))
        }) as HostHandler;

        // Fs.list(path: string) -> List<string> (sorted entry names)
        let list = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            let entries = std::fs::read_dir(&path)
                .map_err(|e| VmError::exception(format!("Fs.list: {e}")))?;
            let mut names = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| VmError::exception(format!("Fs.list: {e}")))?;
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            Ok(Value::list(names.into_iter().map(Value::string).collect()))
        }) as HostHandler;

        // Fs.remove(path: string) -> () (file first, then empty directory)
        let remove = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            std::fs::remove_file(&path)
                .or_else(|_| std::fs::remove_dir(&path))
                .map_err(|e| VmError::exception(format!("Fs.remove: {e}")))?;
            Ok(Value::Unit)
        }) as HostHandler;

        // Fs.create_dir(path: string) -> () (mkdir -p)
        let create_dir = Box::new(|ability: &SuspendedAbility| {
            let path = arg_string(&ability.args, 0)?;
            std::fs::create_dir_all(&path)
                .map_err(|e| VmError::exception(format!("Fs.create_dir: {e}")))?;
            Ok(Value::Unit)
        }) as HostHandler;

        vec![
            (METHOD_READ, read),
            (METHOD_WRITE, write),
            (METHOD_READ_BYTES, read_bytes),
            (METHOD_WRITE_BYTES, write_bytes),
            (METHOD_EXISTS, exists),
            (METHOD_LIST, list),
            (METHOD_REMOVE, remove),
            (METHOD_CREATE_DIR, create_dir),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestType;

    struct TestTypeFactory;

    impl TypeFactory<TestType> for TestTypeFactory {
        fn unit(&self) -> TestType {
            TestType
        }
        fn bool(&self) -> TestType {
            TestType
        }
        fn number(&self) -> TestType {
            TestType
        }
        fn string(&self) -> TestType {
            TestType
        }
        fn bytes(&self) -> TestType {
            TestType
        }
        fn never(&self) -> TestType {
            TestType
        }
        fn type_var(&self) -> TestType {
            TestType
        }
        fn list(&self, _: TestType) -> TestType {
            TestType
        }
    }

    /// Invoke a handler by method ID with the given args.
    fn call(method_id: u16, args: Vec<Value>) -> Result<Value, VmError> {
        let fs = FsRuntimeAbility::new();
        let handlers = fs.handlers();
        let (_, handler) = handlers.iter().find(|(id, _)| *id == method_id).unwrap();
        handler(&SuspendedAbility {
            ability_id: ability_id(),
            method_id,
            args,
        })
    }

    /// A unique temp path for this test run.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ambient_fs_unit_{}_{name}", std::process::id()))
    }

    #[test]
    fn test_fs_ability_constants() {
        assert_eq!(METHOD_READ, 0x0000);
        assert_eq!(METHOD_WRITE, 0x0001);
        assert_eq!(METHOD_READ_BYTES, 0x0002);
        assert_eq!(METHOD_WRITE_BYTES, 0x0003);
        assert_eq!(METHOD_EXISTS, 0x0004);
        assert_eq!(METHOD_LIST, 0x0005);
        assert_eq!(METHOD_REMOVE, 0x0006);
        assert_eq!(METHOD_CREATE_DIR, 0x0007);
        // Identity is stable across calls.
        assert_eq!(ability_id(), FsAbility::ability_id());
    }

    #[test]
    fn test_fs_runtime_ability_name() {
        let fs = FsRuntimeAbility::new();
        assert_eq!(fs.name(), "Fs");
        assert_eq!(fs.ability_id(), ability_id());
    }

    #[test]
    fn test_fs_descriptor_methods() {
        let fs = FsRuntimeAbility::new();
        let factory = TestTypeFactory;
        let descriptor = fs.descriptor(&factory);

        assert_eq!(descriptor.id, ability_id());
        assert_eq!(descriptor.name, "Fs");
        assert_eq!(descriptor.methods.len(), 8);

        let method_names: Vec<_> = descriptor.methods.iter().map(|m| m.name).collect();
        assert!(method_names.contains(&"read"));
        assert!(method_names.contains(&"write"));
        assert!(method_names.contains(&"read_bytes"));
        assert!(method_names.contains(&"write_bytes"));
        assert!(method_names.contains(&"exists"));
        assert!(method_names.contains(&"list"));
        assert!(method_names.contains(&"remove"));
        assert!(method_names.contains(&"create_dir"));
    }

    #[test]
    fn test_fs_handlers_count() {
        let fs = FsRuntimeAbility::new();
        assert_eq!(fs.handlers().len(), 8);
    }

    #[test]
    fn test_fs_write_read_roundtrip() {
        let path = temp_path("roundtrip.txt");
        let path_str = path.to_string_lossy().into_owned();

        let result = call(
            METHOD_WRITE,
            vec![Value::string(&*path_str), Value::string("hello fs")],
        );
        assert_eq!(result.unwrap(), Value::Unit);

        let result = call(METHOD_READ, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::string("hello fs"));

        let result = call(METHOD_EXISTS, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(true));

        call(METHOD_REMOVE, vec![Value::string(&*path_str)]).unwrap();
        let result = call(METHOD_EXISTS, vec![Value::string(&*path_str)]).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn test_fs_read_missing_file_is_catchable_exception() {
        let path = temp_path("does_not_exist.txt");
        let result = call(METHOD_READ, vec![Value::string(path.to_string_lossy())]);
        match result {
            Err(VmError::Exception(_)) => {}
            other => panic!("expected catchable exception, got {other:?}"),
        }
    }

    #[test]
    fn test_fs_type_mismatch_is_type_error() {
        let result = call(METHOD_READ, vec![Value::Number(42.0)]);
        assert!(matches!(result, Err(VmError::TypeErrorOwned { .. })));
    }
}
