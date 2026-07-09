//! The platform bindings interface (`platform.ab`) is the sole source of
//! truth for the native platform.
//!
//! Type checking and compilation resolve performs through the parsed
//! declarations, and host handlers bind by method name against the same
//! resolved interfaces (method IDs are declaration indices). These tests
//! pin the shape of that contract: the declarations must resolve cleanly
//! and expose the ability and method names the handler sets expect, in
//! declaration order.

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

    let mut module = ambient_parser::parse(ambient_platform::ABILITY_DECLARATIONS)
        .expect("platform bindings interface must parse");
    let (abilities, errors) =
        ambient_engine::infer::resolve_ability_declarations(&mut module, &registry);
    assert!(
        errors.is_empty(),
        "platform bindings interface must resolve: {errors:?}"
    );
    abilities
        .into_iter()
        .map(|a| (a.name.to_string(), a))
        .collect()
}

#[test]
fn declarations_expose_the_expected_interfaces() {
    let prelude = resolved_prelude();

    let expected: [(&str, &[&str]); 9] = [
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
    ];

    assert_eq!(
        prelude.len(),
        expected.len(),
        "platform.ab must declare exactly the 9 platform abilities"
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

        for (index, method) in ability.methods.iter().enumerate() {
            assert_eq!(
                usize::from(method.id),
                index,
                "{name}.{} must get its declaration index as its method ID",
                method.name
            );
        }
    }
}

/// Ability IDs are load-bearing at runtime: host handlers bind by id, so a
/// change to how the resolver seeds types (e.g. the primitive nominals it
/// hashes against) must not shift a single id. Pin the content hash of every
/// platform ability. If one of these changes, ability dispatch silently
/// breaks against pre-existing hosts — treat it as a compatibility break, not
/// a test to update. These are byte-identical before and after fully
/// modularizing the primitives (Phase 3).
#[test]
fn ability_ids_are_byte_stable() {
    let prelude = resolved_prelude();

    let golden: [(&str, &str); 9] = [
        (
            "Env",
            "1a40c537d00e9b63a4d9bb213f559b3c57310cd323c54f7c4e70b1039386feee",
        ),
        (
            "Execute",
            "b020b1a50fc352cb302d4484597de11765b8c0a47b50fa8b9fcfcc8de90192ad",
        ),
        (
            "FileSystem",
            "952de97b2d7cf66db1d0b0bfc07982e9da1732fd8ea2b05734ac051dc1af8dfc",
        ),
        (
            "Log",
            "9480648104346749133ec2a4a001a2cedf4ebc7c1ae9862ad4fbc9c951047d43",
        ),
        (
            "Network",
            "94c2e2eedb09e198c7e00be9c95eb850e54580c034f9734a20e4e2a2a1ba9d76",
        ),
        (
            "Process",
            "2c55c28bc25d83f66d415ca01b9811c95912830da92506a78ddb043d73f558ed",
        ),
        (
            "Random",
            "dc771566d8d699804bb1f37b07e76c9300aa43688bea2374f791c7841202b793",
        ),
        (
            "Stdio",
            "7b0924e9cfd5da0b1b7370f3362a7d0af588b39286fca79c1e77fd74e7539ac1",
        ),
        (
            "Time",
            "9716bd900489a788940d3a76c41a30fca3162384bfdf34156649122d5a4b734c",
        ),
    ];

    for (name, expected_hex) in golden {
        let ability = prelude
            .get(name)
            .unwrap_or_else(|| panic!("platform.ab must declare {name}"));
        assert_eq!(
            ability.id.to_hex(),
            expected_hex,
            "{name}'s ability id must be byte-stable"
        );
    }
}

/// `Exception` is declared in Ambient source (`core::exception`) rather than
/// as an engine builtin, but the VM's throw/unwind path still keys on the
/// content hash `ambient_core::exception::ability_id()`. The declaration must
/// therefore reproduce that id byte-for-byte — otherwise a `throw` compiles to
/// a perform the VM no longer recognizes as an exception. Resolve it through
/// the same path a real check uses and pin it to the VM's anchor.
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
    assert_eq!(exception.methods.len(), 1);
    assert_eq!(exception.methods[0].name.as_ref(), "throw");
}
