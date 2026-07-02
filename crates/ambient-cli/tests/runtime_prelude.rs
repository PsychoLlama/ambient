//! The runtime bindings interface (`runtime.ab`) must hash identically
//! to the Rust descriptors it replaces.
//!
//! Host handlers are registered under the descriptor identities, while
//! type checking and compilation resolve performs through the parsed
//! declarations — identical hashes are what let both halves meet. When
//! the descriptors are retired, these assertions go with them.

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
fn declarations_hash_identically_to_descriptors() {
    let prelude = resolved_prelude();
    let expected = [
        ("Console", ambient_runtime::console::ability_id()),
        ("Time", ambient_runtime::time::ability_id()),
        ("Random", ambient_runtime::random::ability_id()),
        ("Log", ambient_runtime::log::ability_id()),
        ("Fs", ambient_runtime::fs::ability_id()),
        ("Network", ambient_runtime::network::ability_id()),
        ("Execute", ambient_runtime::execute::ability_id()),
    ];

    assert_eq!(prelude.len(), expected.len());
    for (name, descriptor_id) in expected {
        let declared = prelude
            .get(name)
            .unwrap_or_else(|| panic!("runtime.ab must declare {name}"));
        assert_eq!(
            declared.id,
            descriptor_id,
            "declaration of {name} must hash like its descriptor \
             (declared {}, descriptor {})",
            declared.id.to_hex(),
            descriptor_id.to_hex()
        );
    }
}

#[test]
fn method_ids_match_descriptor_constants() {
    let prelude = resolved_prelude();
    let method_id = |ability: &str, method: &str| {
        prelude[ability]
            .method(method)
            .unwrap_or_else(|| panic!("{ability} must declare {method}"))
            .id
    };

    use ambient_runtime::{console, execute, fs, log, network, random, time};

    assert_eq!(method_id("Console", "print"), console::METHOD_PRINT);
    assert_eq!(method_id("Console", "eprint"), console::METHOD_EPRINT);
    assert_eq!(method_id("Console", "println"), console::METHOD_PRINTLN);

    assert_eq!(method_id("Time", "now"), time::METHOD_NOW);
    assert_eq!(method_id("Time", "wait"), time::METHOD_WAIT);

    assert_eq!(method_id("Random", "seed"), random::METHOD_SEED);
    assert_eq!(method_id("Random", "in_range"), random::METHOD_IN_RANGE);

    assert_eq!(method_id("Log", "debug"), log::METHOD_DEBUG);
    assert_eq!(method_id("Log", "info"), log::METHOD_INFO);
    assert_eq!(method_id("Log", "warn"), log::METHOD_WARN);
    assert_eq!(method_id("Log", "error"), log::METHOD_ERROR);

    assert_eq!(method_id("Fs", "read"), fs::METHOD_READ);
    assert_eq!(method_id("Fs", "write"), fs::METHOD_WRITE);
    assert_eq!(method_id("Fs", "read_bytes"), fs::METHOD_READ_BYTES);
    assert_eq!(method_id("Fs", "write_bytes"), fs::METHOD_WRITE_BYTES);
    assert_eq!(method_id("Fs", "exists"), fs::METHOD_EXISTS);
    assert_eq!(method_id("Fs", "list"), fs::METHOD_LIST);
    assert_eq!(method_id("Fs", "remove"), fs::METHOD_REMOVE);
    assert_eq!(method_id("Fs", "create_dir"), fs::METHOD_CREATE_DIR);

    assert_eq!(method_id("Network", "listen"), network::METHOD_LISTEN);
    assert_eq!(method_id("Network", "accept"), network::METHOD_ACCEPT);
    assert_eq!(
        method_id("Network", "close_listener"),
        network::METHOD_CLOSE_LISTENER
    );
    assert_eq!(method_id("Network", "connect"), network::METHOD_CONNECT);
    assert_eq!(method_id("Network", "close"), network::METHOD_CLOSE);
    assert_eq!(method_id("Network", "send"), network::METHOD_SEND);
    assert_eq!(method_id("Network", "receive"), network::METHOD_RECEIVE);
    assert_eq!(
        method_id("Network", "local_addr"),
        network::METHOD_LOCAL_ADDR
    );
    assert_eq!(method_id("Network", "peer_addr"), network::METHOD_PEER_ADDR);

    assert_eq!(
        method_id("Execute", "has_function"),
        execute::METHOD_HAS_FUNCTION
    );
    assert_eq!(
        method_id("Execute", "get_dependencies"),
        execute::METHOD_GET_DEPENDENCIES
    );
    assert_eq!(
        method_id("Execute", "load_functions"),
        execute::METHOD_LOAD_FUNCTIONS
    );
    assert_eq!(method_id("Execute", "run"), execute::METHOD_RUN);
    assert_eq!(
        method_id("Execute", "get_functions"),
        execute::METHOD_GET_FUNCTIONS
    );
    assert_eq!(method_id("Execute", "run_with"), execute::METHOD_RUN_WITH);
}
