//! End-to-end coverage of `extern fn`: host bindings, the two-way
//! declaration/binding contract, content addressing (UUID identity, rename
//! stability), the VM dispatch path (direct and first-class calls), and
//! the loud unbound-native failure mode.
//!
//! These drive the embedder API exactly as a third party would: declare
//! signatures in `.ab` source, register a [`NativeRegistry`] through
//! [`BuildOptions::natives`], and wire implementations onto a VM.

use std::fs;
use std::sync::Arc;

use ambient_engine::Value;
use ambient_engine::ast::Module;
use ambient_engine::build::{BuildError, BuildOptions, BuildResult, ParseFailure, build_package};
use ambient_engine::module_path::ModulePath;
use ambient_engine::natives::NativeRegistry;
use ambient_engine::vm::Vm;
use tempfile::TempDir;
use uuid::Uuid;

const DOUBLE_UUID: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_0001);
const GREET_UUID: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_0002);

fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// Write a single-module package with `main.ab` = `source`.
fn temp_package(source: &str) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"externs\"\nversion = \"0.1.0\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(src.join("main.ab"), source).expect("write main.ab");
    dir
}

fn main_module() -> ModulePath {
    ModulePath::from_str_segments(&["main"]).expect("main path")
}

/// A registry binding `double` (×2) and `greet` (prepends "hello, ").
fn test_natives() -> NativeRegistry {
    let mut natives = NativeRegistry::new();
    natives.register(
        &main_module(),
        "double",
        DOUBLE_UUID,
        1,
        Arc::new(|args: Vec<Value>| {
            let Some(Value::Number(n)) = args.first() else {
                panic!("double expects a number");
            };
            Ok(Value::Number(n * 2.0))
        }),
    );
    natives.register(
        &main_module(),
        "greet",
        GREET_UUID,
        1,
        Arc::new(|args: Vec<Value>| {
            let Some(Value::String(s)) = args.first() else {
                panic!("greet expects a string");
            };
            Ok(Value::string(format!("hello, {s}")))
        }),
    );
    natives
}

/// Build a package with the test natives bound (plus the platform stubs,
/// which satisfy `core::system`'s own extern contract).
fn build(dir: &TempDir, natives: &NativeRegistry) -> Result<BuildResult, BuildError> {
    let mut with_stubs = ambient_platform::stub_natives();
    with_stubs.merge(natives);
    build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&with_stubs),
            ..Default::default()
        },
    )
}

/// Wire a VM with the build's code and the embedder natives, and call the
/// entry function.
fn run_entry(result: &BuildResult, natives: &NativeRegistry) -> Result<Value, String> {
    let mut vm = Vm::new();
    vm.register_natives(natives);
    for func in result.compiled.functions.values() {
        vm.load_function(func.clone());
    }
    for (hash, object) in &result.compiled.objects {
        if let Some(value) = object.as_value() {
            vm.load_value(*hash, value);
        }
        if let Some((uuid, param_count)) = object.as_native() {
            vm.load_native(*hash, uuid, param_count);
        }
    }
    let entry = result
        .compiled
        .entry_point
        .expect("package declares a `run` entry");
    vm.call(&entry, vec![]).map_err(|e| e.to_string())
}

const BASIC: &str = "\
extern fn double(x: Number): Number;
extern fn greet(name: String): String;

pub fn run(): Number {
    double(21)
}
";

/// A module declaring only `double` — for contract-violation tests.
const DOUBLE_ONLY: &str = "\
extern fn double(x: Number): Number;

pub fn run(): Number {
    double(21)
}
";

#[test]
fn extern_fn_compiles_binds_and_runs() {
    let dir = temp_package(BASIC);
    let natives = test_natives();
    let result = build(&dir, &natives).expect("build succeeds");
    let value = run_entry(&result, &natives).expect("run succeeds");
    assert_eq!(value, Value::Number(42.0));
}

#[test]
fn extern_fn_is_a_first_class_value() {
    // Bind the extern to a local and call it indirectly: the closure path
    // must dispatch natives exactly like the direct-call path. This is the
    // capability intrinsics never had ("intrinsics are not first-class").
    let dir = temp_package(
        "\
extern fn double(x: Number): Number;
extern fn greet(name: String): String;

fn apply(f: (Number) -> Number, x: Number): Number {
    f(x)
}

pub fn run(): Number {
    let d = double;
    apply(d, 10) + double(1)
}
",
    );
    let natives = test_natives();
    let result = build(&dir, &natives).expect("build succeeds");
    let value = run_entry(&result, &natives).expect("run succeeds");
    assert_eq!(value, Value::Number(22.0));
}

#[test]
fn unbound_extern_fails_the_build() {
    let dir = temp_package(DOUBLE_ONLY);
    let Err(err) = build(&dir, &NativeRegistry::new()) else {
        panic!("must fail");
    };
    let message = err.to_string();
    assert!(
        message.contains("double") && message.contains("no native binding"),
        "expected an unbound-extern error, got: {message}"
    );
}

#[test]
fn arity_mismatch_between_binding_and_declaration_fails() {
    // Declared with two parameters, bound with one.
    let dir = temp_package(
        "\
extern fn double(x: Number, y: Number): Number;

pub fn run(): Number {
    double(21, 0)
}
",
    );
    let Err(err) = build(&dir, &test_natives()) else {
        panic!("must fail");
    };
    let message = err.to_string();
    assert!(
        message.contains("double") && message.contains("2") && message.contains("1"),
        "expected an arity-contract error, got: {message}"
    );
}

#[test]
fn dangling_binding_fails_the_build() {
    // The registry binds `greet`, but the module declares only `double`:
    // a stale host table must fail loudly, not silently dispatch nothing.
    let dir = temp_package(DOUBLE_ONLY);
    let Err(err) = build(&dir, &test_natives()) else {
        panic!("must fail");
    };
    let message = err.to_string();
    assert!(
        message.contains("greet") && message.contains("no extern fn declaration"),
        "expected a dangling-binding error, got: {message}"
    );
}

#[test]
fn renaming_an_extern_fn_preserves_its_hash_and_callers() {
    // Same UUID bound under two names: the native object's hash must not
    // move (the name never enters the encoding), so a rename is free.
    let dir_a = temp_package(DOUBLE_ONLY);
    let mut natives_a = NativeRegistry::new();
    natives_a.register(
        &main_module(),
        "double",
        DOUBLE_UUID,
        1,
        Arc::new(|args: Vec<Value>| {
            let Some(Value::Number(n)) = args.first() else {
                panic!("expects a number");
            };
            Ok(Value::Number(n * 2.0))
        }),
    );
    let result_a = build(&dir_a, &natives_a).expect("build a");

    let dir_b = temp_package(
        "\
extern fn twice(x: Number): Number;

pub fn run(): Number {
    twice(21)
}
",
    );
    let mut natives_b = NativeRegistry::new();
    natives_b.register(
        &main_module(),
        "twice",
        DOUBLE_UUID,
        1,
        Arc::new(|args: Vec<Value>| {
            let Some(Value::Number(n)) = args.first() else {
                panic!("expects a number");
            };
            Ok(Value::Number(n * 2.0))
        }),
    );
    let result_b = build(&dir_b, &natives_b).expect("build b");

    // The merged build also carries core's natives; select the test one.
    let native_hash = |result: &BuildResult| {
        let hashes: Vec<blake3::Hash> = result
            .compiled
            .objects
            .iter()
            .filter(|(_, object)| {
                object
                    .as_native()
                    .is_some_and(|(uuid, _)| uuid == DOUBLE_UUID)
            })
            .map(|(hash, _)| *hash)
            .collect();
        assert_eq!(hashes.len(), 1, "exactly one test native object");
        hashes[0]
    };
    assert_eq!(
        native_hash(&result_a),
        native_hash(&result_b),
        "renaming an extern fn must not change its content hash"
    );

    // The callers are byte-identical too: `run`'s hash covers only the
    // callee's (unchanged) hash, never its spelled name.
    assert_eq!(
        result_a.compiled.entry_point, result_b.compiled.entry_point,
        "a caller's hash must survive its callee's rename"
    );
}

#[test]
fn calling_an_unbound_native_fails_loudly_at_runtime() {
    // Compile with the binding, but execute on a VM that lacks the
    // implementation — the remote-host story. The failure names the uuid.
    let dir = temp_package(BASIC);
    let natives = test_natives();
    let result = build(&dir, &natives).expect("build succeeds");
    let err = run_entry(&result, &NativeRegistry::new()).expect_err("must fail");
    assert!(
        err.contains("no native implementation") && err.contains(&DOUBLE_UUID.to_string()),
        "expected an unbound-native runtime error, got: {err}"
    );
}

#[test]
fn extern_fn_ships_through_a_pack() {
    // Natives ride the dependency closure through pack encode/decode, so
    // shipped code arrives with the identities it needs to bind.
    let dir = temp_package(BASIC);
    let natives = test_natives();
    let result = build(&dir, &natives).expect("build succeeds");

    let pack = result.compiled.to_pack();
    let decoded = ambient_engine::store::Pack::decode(&pack.encode()).expect("pack roundtrip");
    let module = ambient_engine::compiler::CompiledModule::from_pack(&decoded).expect("from pack");

    // The merged build also ships core's natives; check the test ones.
    let natives_in_pack: Vec<_> = module
        .objects
        .values()
        .filter_map(ambient_engine::object::StoredObject::as_native)
        .collect();
    for expected in [(DOUBLE_UUID, 1), (GREET_UUID, 1)] {
        assert!(
            natives_in_pack.contains(&expected),
            "pack must carry the test natives, got {natives_in_pack:?}"
        );
    }

    // And the reconstructed module still runs.
    let rebuilt = BuildResult {
        compiled: module,
        module_count: result.module_count,
        package_name: result.package_name.clone(),
        link_table: result.link_table.clone(),
        interfaces: result.interfaces.clone(),
        dispatch_surface_hash: result.dispatch_surface_hash,
    };
    let value = run_entry(&rebuilt, &natives).expect("run succeeds");
    assert_eq!(value, Value::Number(42.0));
}
