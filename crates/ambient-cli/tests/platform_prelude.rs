//! The platform bindings interface (`platform.ab`) is the sole source of
//! truth for the native platform.
//!
//! Type checking and compilation resolve performs through the parsed
//! declarations. These tests pin the shape of that contract: the
//! declarations must resolve cleanly, expose the expected ability and
//! method names, and keep their reserved uuids (the identities every
//! compiled perform and shipped handler key on) byte-stable.

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::ability_resolver::DynAbility;

fn resolved_prelude() -> HashMap<String, Arc<DynAbility>> {
    // Parse-only core registry: supplies the prelude so ability resolution
    // seeds the primitive nominals its signatures hash against.
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules must parse");

    // The interface is a module tree now (one submodule per ability). Register
    // the whole tree so cross-module references resolve (`Log` performs
    // `core::system::Stdio`), then read the ability declarations back out of
    // the registry — exactly how a real check discovers them.
    ambient_engine::core_library::register_declaration_modules(
        &mut registry,
        ambient_platform::platform_modules(),
        |s| ambient_parser::parse(s).map_err(|e| e.to_string()),
    )
    .expect("platform modules must parse");

    ambient_engine::infer::resolve_registry_abilities(&registry)
        .into_iter()
        .filter(|(fqn, _)| fqn.module.module_path_string().starts_with("core::system"))
        .map(|(_, ability)| (ability.name.to_string(), ability))
        .collect()
}

#[test]
fn declarations_expose_the_expected_interfaces() {
    let prelude = resolved_prelude();

    let expected: [(&str, &[&str]); 11] = [
        ("Stdio", &["out", "err", "read"]),
        ("Time", &["now", "wait"]),
        ("Random", &["seed", "in_range"]),
        ("Log", &["debug", "info", "warn", "error"]),
        (
            "FileSystem",
            &[
                "read",
                "write",
                "read_binary",
                "write_binary",
                "exists",
                "list",
                "remove",
                "create_dir",
            ],
        ),
        (
            "Network",
            &[
                "listen",
                "accept",
                "close_listener",
                "connect",
                "close",
                "send",
                "receive",
                "local_addr",
                "peer_addr",
            ],
        ),
        (
            "Process",
            &["spawn", "send", "send_named", "self_pid", "whereis", "exit"],
        ),
        (
            "Execute",
            &[
                "has_function",
                "get_dependencies",
                "load_functions",
                "run",
                "get_functions",
                "run_with",
            ],
        ),
        ("Env", &["var", "vars", "set", "args", "cwd", "pid"]),
        ("Live", &["latest"]),
        ("State", &["init", "get", "set", "update"]),
    ];

    assert_eq!(
        prelude.len(),
        expected.len(),
        "platform.ab must declare exactly the 11 platform abilities"
    );

    for (name, methods) in expected {
        let ability = prelude
            .get(name)
            .unwrap_or_else(|| panic!("platform.ab must declare {name}"));

        let declared: Vec<&str> = ability.methods.iter().map(|m| m.name.as_ref()).collect();
        assert_eq!(
            declared, methods,
            "{name} must declare these methods in this order \
             (method ID = declaration index)"
        );

        for method in &ability.methods {
            assert!(
                method.has_impl,
                "{name}.{} must carry a default implementation",
                method.name
            );
        }
    }
}

/// Ability identities are load-bearing at runtime: compiled performs and
/// shipped handlers key on the uuid-derived id, so the declarations must
/// keep their reserved uuids (block `FFFFFFFF-FFFF-FFFF-FFFD-…`). If one of
/// these changes, ability dispatch silently breaks against pre-existing
/// code — treat it as a compatibility break, not a test to update.
#[test]
fn ability_uuids_are_pinned() {
    let prelude = resolved_prelude();

    let reserved: [(&str, u128); 11] = [
        ("Stdio", 0x2),
        ("Time", 0x3),
        ("Random", 0x4),
        ("Log", 0x5),
        ("FileSystem", 0x6),
        ("Network", 0x7),
        ("Process", 0x8),
        ("Env", 0x9),
        ("Execute", 0xA),
        ("Live", 0xB),
        ("State", 0xC),
    ];

    for (name, slot) in reserved {
        let ability = prelude
            .get(name)
            .unwrap_or_else(|| panic!("platform.ab must declare {name}"));
        let expected = uuid::Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFD_0000_0000_0000 + slot);
        assert_eq!(
            ability.uuid, expected,
            "{name}'s reserved uuid must be pinned"
        );
        assert_eq!(
            ability.id,
            ambient_core::AbilityId::from_uuid(&expected),
            "{name}'s ability id must derive from its uuid"
        );
    }
}

/// Canonical signature hashes are the second `MethodKey` input, so the
/// rendering (via the seeded inference context) must stay byte-stable: a
/// drift re-keys every platform method at once. Spot-pin a monomorphic and
/// a generic signature.
#[test]
fn signature_hashes_are_byte_stable() {
    use ambient_core::SignatureHash;

    let prelude = resolved_prelude();

    let stdio = prelude.get("Stdio").expect("Stdio");
    let out = stdio.method("out").expect("Stdio::out");
    assert_eq!(
        out.signature,
        SignatureHash::new(&["string"], "unit"),
        "Stdio::out must render (string) -> unit"
    );

    let execute = prelude.get("Execute").expect("Execute");
    let run = execute.method("run").expect("Execute::run");
    assert_eq!(
        run.signature,
        SignatureHash::new(&["string", "var0"], "var1"),
        "Execute::run<T, R> must render position-canonical type variables"
    );

    // `Time::wait(duration: Duration): ()`. `Duration` is a cross-module
    // nominal (`core::time::Duration`): an ability signature resolves before
    // the module's alias table is populated, so it stays an unresolved
    // `Named` with no uuid and renders by its name — the documented,
    // byte-stable fallback of the uuid-based nominal rendering. (A uuid would
    // be rename-stable, but this path never resolves one to render.)
    let time = prelude.get("Time").expect("Time");
    let wait = time.method("wait").expect("Time::wait");
    assert_eq!(
        wait.signature,
        SignatureHash::new(&["named:Duration"], "unit"),
        "Time::wait must render an unresolved cross-module nominal by name"
    );
}

/// `Exception` is declared in Ambient source (`core::exception`) rather than
/// as an engine builtin, but the VM's throw/unwind path still keys on the
/// anchors in `ambient_core::exception` (the reserved uuid and the derived
/// `throw` method key). The declaration must reproduce them byte-for-byte —
/// otherwise a `throw` compiles to a perform the VM no longer recognizes as
/// an exception. Resolve it through the same path a real check uses and pin
/// it to the VM's anchors.
#[test]
fn declared_exception_reproduces_the_vm_anchor_id() {
    let mut registry = ambient_engine::module_registry::ModuleRegistry::new();
    ambient_engine::core_library::register_core_modules(&mut registry, |s| {
        ambient_parser::parse(s).map_err(|e| e.to_string())
    })
    .expect("core modules must parse");

    let exception = ambient_engine::infer::resolve_registry_abilities(&registry)
        .into_iter()
        .find(|(fqn, _)| fqn.name() == "Exception")
        .map(|(_, ability)| ability)
        .expect("core must declare `Exception`");

    assert_eq!(
        exception.id,
        ambient_core::exception::ability_id(),
        "core::exception::Exception must reproduce the VM's Exception id"
    );
    assert_eq!(exception.uuid, ambient_core::exception::EXCEPTION_UUID);
    assert_eq!(exception.methods.len(), 1);
    let throw = &exception.methods[0];
    assert_eq!(throw.name.as_ref(), "throw");
    assert!(
        !throw.has_impl,
        "`throw` is abstract (never-returning; unhandled = uncaught)"
    );
    assert_eq!(
        ambient_core::MethodKey::derive(&exception.uuid, &throw.signature, None),
        ambient_core::exception::throw_method_key(),
        "the declared signature must reproduce the VM's throw method key"
    );
}
