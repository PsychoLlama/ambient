//! `Env` ability - the host process's environment.
//!
//! Variables, arguments, working directory, and OS process id. All but
//! `args` read (or mutate) live OS state directly and need no captured
//! configuration; `args` returns argv threaded in from the CLI at
//! startup, because the OS process arguments are not the program's
//! logical arguments (the CLI composes them: program path at index 0,
//! then the user args after `--`).
//!
//! # API
//!
//! - `var(name: string) -> Option<string>` - a variable's value (None if unset)
//! - `vars() -> List<(string, string)>` - every variable as (name, value) pairs
//! - `set(name: string, value: string) -> ()` - set/overwrite a variable
//! - `args() -> List<string>` - the captured argv (index 0 is the program path)
//! - `cwd() -> string` - the current working directory
//! - `pid() -> number` - the OS process id
//!
//! # Errors
//!
//! A missing variable is `None`, not an exception: absence is normal
//! data (matching the "no `Result`/exception for domain absence" rule in
//! `ref/architecture.md`). `cwd` raises a catchable [`VmError::exception`]
//! when the working directory can't be read. Argument-type mismatches are
//! programmer errors and remain fatal type errors.

use std::sync::Arc;

use ambient_ability::{SuspendedAbility, Value, VmError};
use ambient_engine::ability_resolver::AbilityInterface;
use ambient_engine::vm::Vm;

use crate::{extract_string, require};

/// `Env.var(name: string) -> Option<string>` (None if unset)
fn var(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let name = extract_string(&ability.args)?;
    Ok(match std::env::var(&name) {
        Ok(value) => Value::some(Value::string(value)),
        Err(_) => Value::none(),
    })
}

/// `Env.vars() -> List<(string, string)>`
#[allow(clippy::unnecessary_wraps)]
fn vars(_ability: &SuspendedAbility) -> Result<Value, VmError> {
    let pairs = std::env::vars()
        .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
        .collect();
    Ok(Value::list(pairs))
}

/// `Env.set(name: string, value: string) -> ()` (process-global, best-effort)
fn set(ability: &SuspendedAbility) -> Result<Value, VmError> {
    let name = extract_string(&ability.args)?;
    let value = match ability.args.get(1) {
        Some(Value::String(s)) => s.as_ref().clone(),
        other => {
            return Err(VmError::TypeErrorOwned {
                expected: "String".to_string(),
                got: other
                    .map_or("missing argument", Value::type_name)
                    .to_string(),
            });
        }
    };
    // SAFETY: Under edition 2024 `set_var` is `unsafe` because mutating the
    // process environment while another thread reads it is undefined
    // behavior. Ambient runs each process on its own OS thread, so this is
    // inherently process-global and best-effort — intended for early
    // startup/config use, not concurrent mutation. This is a known
    // limitation, not a soundness guarantee.
    unsafe {
        std::env::set_var(&name, &value);
    }
    Ok(Value::Unit)
}

/// `Env.cwd() -> string` (raises if the working directory can't be read)
fn cwd(_ability: &SuspendedAbility) -> Result<Value, VmError> {
    let dir = std::env::current_dir().map_err(|e| VmError::exception(format!("Env.cwd: {e}")))?;
    Ok(Value::string(dir.to_string_lossy().into_owned()))
}

/// `Env.pid() -> number` (the OS process id)
#[allow(clippy::unnecessary_wraps)]
fn pid(_ability: &SuspendedAbility) -> Result<Value, VmError> {
    Ok(Value::Number(f64::from(std::process::id())))
}

/// Register the `Env` ability handlers on a VM.
///
/// `var`/`vars`/`set`/`cwd`/`pid` read or mutate the OS live; `args`
/// returns the captured `argv` (the CLI's program path at index 0
/// followed by the user args). The REPL and tests pass an empty `argv`.
///
/// # Panics
///
/// Panics if the resolved interface is missing an expected method — the
/// bindings interface and this handler set have drifted.
pub fn register_env(vm: &mut Vm, ability: &AbilityInterface, argv: Arc<Vec<String>>) {
    vm.register_host_handler(ability.id, require(ability, "var"), Box::new(var));
    vm.register_host_handler(ability.id, require(ability, "vars"), Box::new(vars));
    vm.register_host_handler(ability.id, require(ability, "set"), Box::new(set));
    vm.register_host_handler(
        ability.id,
        require(ability, "args"),
        Box::new(move |_ability: &SuspendedAbility| {
            Ok(Value::list(
                argv.iter().cloned().map(Value::string).collect(),
            ))
        }),
    );
    vm.register_host_handler(ability.id, require(ability, "cwd"), Box::new(cwd));
    vm.register_host_handler(ability.id, require(ability, "pid"), Box::new(pid));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_core::AbilityId;

    type Handler = fn(&SuspendedAbility) -> Result<Value, VmError>;

    fn call(handler: Handler, args: Vec<Value>) -> Result<Value, VmError> {
        handler(&SuspendedAbility {
            ability_id: AbilityId::from_bytes([6; 32]),
            method_id: 0,
            args,
        })
    }

    #[test]
    fn var_reads_a_set_variable_as_some() {
        let key = format!("AMBIENT_ENV_UNIT_{}", std::process::id());
        // SAFETY: single-threaded unit test; no other thread reads the env.
        unsafe {
            std::env::set_var(&key, "hello");
        }
        let result = call(var, vec![Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::some(Value::string("hello")));
    }

    #[test]
    fn var_reads_an_unset_variable_as_none() {
        let key = format!("AMBIENT_ENV_UNIT_UNSET_{}", std::process::id());
        // SAFETY: single-threaded unit test; no other thread reads the env.
        unsafe {
            std::env::remove_var(&key);
        }
        let result = call(var, vec![Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::none());
    }

    #[test]
    fn set_then_var_roundtrips() {
        let key = format!("AMBIENT_ENV_UNIT_SET_{}", std::process::id());
        let result = call(set, vec![Value::string(&*key), Value::string("world")]).unwrap();
        assert_eq!(result, Value::Unit);
        let result = call(var, vec![Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::some(Value::string("world")));
    }

    #[test]
    fn var_type_mismatch_is_type_error() {
        let result = call(var, vec![Value::Number(1.0)]);
        assert!(matches!(result, Err(VmError::TypeErrorOwned { .. })));
    }

    #[test]
    fn pid_matches_the_process() {
        let result = call(pid, vec![]).unwrap();
        #[allow(clippy::cast_precision_loss)]
        let expected = Value::Number(f64::from(std::process::id()));
        assert_eq!(result, expected);
    }

    #[test]
    fn cwd_returns_a_non_empty_string() {
        let result = call(cwd, vec![]).unwrap();
        match result {
            Value::String(s) => assert!(!s.is_empty()),
            other => panic!("expected string, got {other:?}"),
        }
    }
}
