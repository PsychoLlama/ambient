//! `Env` natives - the host process's environment.
//!
//! Variables, arguments, working directory, and OS process id. All but
//! `env_args` read (or mutate) live OS state directly and need no captured
//! configuration; `env_args` returns argv threaded in from the CLI at
//! startup, because the OS process arguments are not the program's
//! logical arguments (the CLI composes them: program path at index 0,
//! then the user args after `--`).
//!
//! # Errors
//!
//! A missing variable is `None`, not an exception: absence is normal
//! data (matching the "no `Result`/exception for domain absence" rule in
//! `ref/architecture.md`). `env_cwd` raises a catchable
//! [`VmError::exception`] when the working directory can't be read.
//! Argument-type mismatches are programmer errors and remain fatal type
//! errors.

use std::sync::Arc;

use ambient_ability::{Value, VmError};
use ambient_engine::natives::NativeRegistry;

use crate::{bind, extract_string};

/// `env_var(name: string) -> Option<string>` (None if unset)
fn var(args: &[Value]) -> Result<Value, VmError> {
    let name = extract_string(args)?;
    Ok(match std::env::var(&name) {
        Ok(value) => Value::some(Value::string(value)),
        Err(_) => Value::none(),
    })
}

/// `env_vars() -> List<(string, string)>`
#[allow(clippy::unnecessary_wraps)]
fn vars(_args: &[Value]) -> Result<Value, VmError> {
    let pairs = std::env::vars()
        .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
        .collect();
    Ok(Value::list(pairs))
}

/// `env_set(name: string, value: string) -> ()` (process-global, best-effort)
fn set(args: &[Value]) -> Result<Value, VmError> {
    let name = extract_string(args)?;
    let value = match args.get(1) {
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

/// `env_cwd() -> string` (raises if the working directory can't be read)
fn cwd(_args: &[Value]) -> Result<Value, VmError> {
    let dir = std::env::current_dir().map_err(|e| VmError::exception(format!("Env.cwd: {e}")))?;
    Ok(Value::string(dir.to_string_lossy().into_owned()))
}

/// `env_pid() -> number` (the OS process id)
#[allow(clippy::unnecessary_wraps)]
fn pid(_args: &[Value]) -> Result<Value, VmError> {
    Ok(Value::Number(f64::from(std::process::id())))
}

/// The `Env` native implementations.
///
/// `env_var`/`env_vars`/`env_set`/`env_cwd`/`env_pid` read or mutate the
/// OS live; `env_args` returns the captured `argv` (the CLI's program
/// path at index 0 followed by the user args). The REPL and tests pass
/// an empty `argv`.
#[must_use]
pub fn env_natives(argv: Arc<Vec<String>>) -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    bind(
        &mut registry,
        "env_var",
        Arc::new(|args: Vec<Value>| var(&args)),
    );
    bind(
        &mut registry,
        "env_vars",
        Arc::new(|args: Vec<Value>| vars(&args)),
    );
    bind(
        &mut registry,
        "env_set",
        Arc::new(|args: Vec<Value>| set(&args)),
    );
    bind(
        &mut registry,
        "env_args",
        Arc::new(move |_args: Vec<Value>| {
            Ok(Value::list(
                argv.iter().cloned().map(Value::string).collect(),
            ))
        }),
    );
    bind(
        &mut registry,
        "env_cwd",
        Arc::new(|args: Vec<Value>| cwd(&args)),
    );
    bind(
        &mut registry,
        "env_pid",
        Arc::new(|args: Vec<Value>| pid(&args)),
    );
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_reads_a_set_variable_as_some() {
        let key = format!("AMBIENT_ENV_UNIT_{}", std::process::id());
        // SAFETY: single-threaded unit test; no other thread reads the env.
        unsafe {
            std::env::set_var(&key, "hello");
        }
        let result = var(&[Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::some(Value::string("hello")));
    }

    #[test]
    fn var_reads_an_unset_variable_as_none() {
        let key = format!("AMBIENT_ENV_UNIT_UNSET_{}", std::process::id());
        // SAFETY: single-threaded unit test; no other thread reads the env.
        unsafe {
            std::env::remove_var(&key);
        }
        let result = var(&[Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::none());
    }

    #[test]
    fn set_then_var_roundtrips() {
        let key = format!("AMBIENT_ENV_UNIT_SET_{}", std::process::id());
        let result = set(&[Value::string(&*key), Value::string("world")]).unwrap();
        assert_eq!(result, Value::Unit);
        let result = var(&[Value::string(&*key)]).unwrap();
        assert_eq!(result, Value::some(Value::string("world")));
    }

    #[test]
    fn var_type_mismatch_is_type_error() {
        let result = var(&[Value::Number(1.0)]);
        assert!(matches!(result, Err(VmError::TypeErrorOwned { .. })));
    }

    #[test]
    fn pid_matches_the_process() {
        let result = pid(&[]).unwrap();
        #[allow(clippy::cast_precision_loss)]
        let expected = Value::Number(f64::from(std::process::id()));
        assert_eq!(result, expected);
    }

    #[test]
    fn cwd_returns_a_non_empty_string() {
        let result = cwd(&[]).unwrap();
        match result {
            Value::String(s) => assert!(!s.is_empty()),
            other => panic!("expected string, got {other:?}"),
        }
    }

    #[test]
    fn args_returns_the_captured_argv() {
        let registry = env_natives(Arc::new(vec!["prog".into(), "a".into()]));
        let func = registry
            .impl_for(&crate::native_uuid("env_args"))
            .expect("env_args bound");
        let result = func(vec![]).unwrap();
        assert_eq!(
            result,
            Value::list(vec![Value::string("prog"), Value::string("a")])
        );
    }
}
