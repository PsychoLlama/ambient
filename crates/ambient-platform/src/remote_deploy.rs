//! The `deploy_apply` and `deploy_plan` natives: receiving a generation
//! pack and either applying it to the running system or dry-running it.
//!
//! Both natives are thin shims. The real work — decoding the pack,
//! recomputing hashes from bytes, and driving the deploy core — belongs to
//! the embedding host, which owns the deploy core, the task registry, and
//! the store. The host supplies each as a hook ([`DeployApplyHook`],
//! [`DeployPlanHook`]), set once after its runtimes exist (the native set
//! has to be built before them: the VM factory that every runtime captures
//! carries the sets). A perform before a hook is set — or on a host that
//! never sets one, like the Execute sandbox — is a catchable `Err`, not a
//! fault.
//!
//! `apply` and `plan` share their whole surface here (identical argument
//! extraction, identical `Result<String, String>` wrapping, identical
//! "no deploy runtime" error). They differ only in the host hook behind
//! them: `apply` swaps, records, and reconciles under the host's deploy
//! lock; `plan` validates and reports without any of that — see the
//! `Deploy` ability's docs (`platform/deploy.ab`) and the plan core
//! ([`crate::deploy::DeployRuntime::plan`]).

use std::sync::{Arc, OnceLock};

use ambient_ability::{Value, VmError};
use ambient_engine::natives::NativeRegistry;

use crate::{bind, extract_bytes, into_result};

/// The host's deploy entry point: pack bytes and an entry name in, a
/// rendered deploy report out (or why the deploy was rejected).
///
/// The hook must serialize itself against the host's other deploy
/// frontends (the dev loop's watcher, the REPL) — the task registry's
/// reconcile bracket is not reentrant.
pub type DeployApplyHook = Arc<dyn Fn(&[u8], &str) -> Result<String, String> + Send + Sync>;

/// Deferred hook cell: the native set captures it empty; the host fills
/// it once its deploy machinery exists.
pub type DeployApplySlot = Arc<OnceLock<DeployApplyHook>>;

/// The host's deploy *plan* entry point: same inputs as [`DeployApplyHook`],
/// but a dry run — validate and report the would-be diff without swapping
/// the name table, recording a generation, or running the entry. Unlike
/// apply, it takes no deploy lock and is safe from inside a deploy pass, so
/// the host wires it with no reentrancy guard.
pub type DeployPlanHook = Arc<dyn Fn(&[u8], &str) -> Result<String, String> + Send + Sync>;

/// Deferred plan-hook cell (see [`DeployApplySlot`]).
pub type DeployPlanSlot = Arc<OnceLock<DeployPlanHook>>;

/// The `Deploy` native implementations: `deploy_apply(pack, entry)` and
/// `deploy_plan(pack, entry)`, each delegating to its host hook and
/// wrapping the outcome as an in-language `Result<String, String>`.
#[must_use]
pub fn remote_deploy_natives(apply: DeployApplySlot, plan: DeployPlanSlot) -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    bind_hook_native(&mut registry, "deploy_apply", "Deploy.apply", apply);
    bind_hook_native(&mut registry, "deploy_plan", "Deploy.plan", plan);
    registry
}

/// Bind one hook-backed deploy native: extract `(pack, entry)`, look the
/// host hook up in `slot`, and wrap its `Result<String, String>` as an
/// in-language value. `method` names the perform in error messages
/// (`Deploy.apply` / `Deploy.plan`). `apply` and `plan` are identical here
/// but for the hook behind the slot.
fn bind_hook_native(
    registry: &mut NativeRegistry,
    name: &'static str,
    method: &'static str,
    slot: DeployApplySlot,
) {
    bind(
        registry,
        name,
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
                let (pack, entry) = deploy_args(&args)?;
                let hook = slot.get().ok_or_else(|| {
                    VmError::exception(format!("{method}: no deploy runtime on this host"))
                })?;
                let report = hook(&pack, &entry).map_err(VmError::exception)?;
                Ok(Value::string(report))
            })())
        }),
    );
}

/// Extract the `(pack, entry)` pair both deploy natives take: a `Binary`
/// pack and a `String` entry name.
fn deploy_args(args: &[Value]) -> Result<(Vec<u8>, String), VmError> {
    if args.len() < 2 {
        return Err(VmError::TypeErrorOwned {
            expected: "2 arguments (pack, entry)".to_string(),
            got: format!("{} arguments", args.len()),
        });
    }
    let pack = extract_bytes(&args[0])?;
    let entry = match &args[1] {
        Value::String(s) => s.to_string(),
        other => {
            return Err(VmError::TypeErrorOwned {
                expected: "String".to_string(),
                got: other.type_name().to_string(),
            });
        }
    };
    Ok((pack, entry))
}
