//! The runtime bindings interface (`runtime.ab`) is the sole source of
//! truth for the native runtime.
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
    let mut module = ambient_parser::parse(ambient_runtime::ABILITY_DECLARATIONS)
        .expect("runtime bindings interface must parse");
    let (abilities, errors) = ambient_engine::infer::resolve_ability_declarations(&mut module);
    assert!(
        errors.is_empty(),
        "runtime bindings interface must resolve: {errors:?}"
    );
    abilities
        .into_iter()
        .map(|a| (a.name.to_string(), a))
        .collect()
}

#[test]
fn declarations_expose_the_expected_interfaces() {
    let prelude = resolved_prelude();

    let expected: [(&str, &[&str]); 7] = [
        ("Console", &["print", "eprint", "println"]),
        ("Time", &["now", "wait"]),
        ("Random", &["seed", "in_range"]),
        ("Log", &["debug", "info", "warn", "error"]),
        (
            "FileSystem",
            &[
                "read",
                "write",
                "read_bytes",
                "write_bytes",
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
    ];

    assert_eq!(
        prelude.len(),
        expected.len(),
        "runtime.ab must declare exactly the 7 runtime abilities"
    );

    for (name, methods) in expected {
        let ability = prelude
            .get(name)
            .unwrap_or_else(|| panic!("runtime.ab must declare {name}"));

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
