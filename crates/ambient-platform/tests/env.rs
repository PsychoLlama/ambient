//! Integration tests for the `Env` ability's host handlers.
//!
//! These drive the handlers through a real VM (`emit_suspend` + `Perform`,
//! the same path a compiled `core::system::Env::args!()` perform takes),
//! binding against the resolved `platform.ab` interface. `args` is the
//! interesting one: it returns argv the CLI captures at startup rather
//! than live OS state, so we register a known argv and read it back.

use std::sync::Arc;

use ambient_engine::ability_resolver::{AbilityInterface, DynAbility};
use ambient_engine::bytecode::{BytecodeBuilder, Opcode};
use ambient_engine::infer::resolve_ability_declarations;
use ambient_engine::value::Value;
use ambient_engine::vm::Vm;
use ambient_platform::register_env;

/// The resolved `Env` interface from the platform bindings.
fn env_interface() -> AbilityInterface {
    // Parse-only core registry: seeds the primitive nominals ability
    // resolution hashes against, so ids match the CLI's.
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules parse");
    let mut module = ambient_parser::parse(ambient_platform::ABILITY_DECLARATIONS)
        .expect("platform declarations parse");
    let (abilities, errors) = resolve_ability_declarations(&mut module, &registry);
    assert!(errors.is_empty(), "platform declarations resolve");
    abilities
        .iter()
        .find(|a: &&Arc<DynAbility>| a.name.as_ref() == "Env")
        .map(|a| AbilityInterface::from(&**a))
        .expect("platform.ab declares Env")
}

/// Perform `Env::<method>()` (no arguments) on a VM and return the result.
fn perform_nullary(vm: &mut Vm, interface: &AbilityInterface, method: &str) -> Value {
    let method_id = interface
        .method_id(method)
        .unwrap_or_else(|| panic!("Env has method `{method}`"));

    let mut builder = BytecodeBuilder::new();
    builder.emit_suspend(interface.id, method_id, 0);
    builder.emit(Opcode::Perform);
    builder.emit(Opcode::Return);
    let func = builder.build(0, 0);
    let hash = func.hash;
    vm.load_function(func);

    vm.call(&hash, vec![]).expect("perform succeeds")
}

#[test]
fn args_returns_the_captured_argv() {
    let interface = env_interface();
    let argv = Arc::new(vec!["prog".to_string(), "a".to_string(), "b".to_string()]);

    let mut vm = Vm::new();
    register_env(&mut vm, &interface, Arc::clone(&argv));

    let result = perform_nullary(&mut vm, &interface, "args");
    assert_eq!(
        result,
        Value::list(vec![
            Value::string("prog"),
            Value::string("a"),
            Value::string("b"),
        ]),
        "args() returns the captured argv, program path first"
    );
}

#[test]
fn pid_returns_the_process_id() {
    let interface = env_interface();
    let mut vm = Vm::new();
    register_env(&mut vm, &interface, Arc::new(Vec::new()));

    let result = perform_nullary(&mut vm, &interface, "pid");
    match result {
        Value::Number(n) => {
            assert_eq!(n, f64::from(std::process::id()), "pid matches the OS pid");
        }
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn cwd_returns_a_non_empty_string() {
    let interface = env_interface();
    let mut vm = Vm::new();
    register_env(&mut vm, &interface, Arc::new(Vec::new()));

    let result = perform_nullary(&mut vm, &interface, "cwd");
    match result {
        Value::String(s) => assert!(!s.is_empty(), "cwd is a non-empty path"),
        other => panic!("expected a string, got {other:?}"),
    }
}
