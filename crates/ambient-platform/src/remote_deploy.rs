//! The `deploy_apply` native: receiving a generation pack and applying
//! it to the running system.
//!
//! The native itself is a thin shim. The real work — decoding the pack,
//! recomputing hashes from bytes, and driving the full declarative
//! deploy pass (load, validate, swap, reconcile, report) — belongs to
//! the embedding host, which owns the deploy core, the task registry,
//! and the store. The host supplies it as a [`DeployApplyHook`], set
//! once after its runtimes exist (the native set has to be built before
//! them: the VM factory that every runtime captures carries the sets).
//! A perform before the hook is set — or on a host that never sets one,
//! like the Execute sandbox — is a catchable `Err`, not a fault.

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

/// The `Deploy` native implementation: `deploy_apply(pack, entry)`
/// delegates to the host's hook and wraps the outcome as an in-language
/// `Result<String, String>`.
#[must_use]
pub fn remote_deploy_natives(slot: DeployApplySlot) -> NativeRegistry {
    let mut registry = NativeRegistry::new();
    bind(
        &mut registry,
        "deploy_apply",
        Arc::new(move |args: Vec<Value>| {
            into_result((|| {
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
                let hook = slot.get().ok_or_else(|| {
                    VmError::exception("Deploy.apply: no deploy runtime on this host")
                })?;
                let report = hook(&pack, &entry).map_err(VmError::exception)?;
                Ok(Value::string(report))
            })())
        }),
    );
    registry
}
