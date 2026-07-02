//! Content-addressing invariants, verified end-to-end from source.
//!
//! These tests pin the core promise of the language: a function's hash is
//! determined by its implementation and its dependencies — nothing else.
//! Trait impl methods are first-class functions and must uphold the same
//! invariants (they historically did not: their hashes were derived from a
//! declaration-order trait counter and never touched the method body).

use std::collections::HashMap;
use std::sync::Arc;

use ambient_engine::compiler::CompiledModule;

/// Parse, type-check, and compile a single module from source.
fn compile(source: &str) -> CompiledModule {
    let module = ambient_parser::parse(source).expect("source should parse");
    let checked = ambient_engine::infer::check_module(module);
    assert!(
        checked.errors.is_empty(),
        "type errors: {:?}",
        checked.errors
    );
    ambient_engine::compiler::compile_module(&checked.module).expect("source should compile")
}

/// Collect name → hash for every named function in the module.
fn named_hashes(module: &CompiledModule) -> HashMap<Arc<str>, blake3::Hash> {
    module
        .function_names
        .iter()
        .map(|(name, hash)| (Arc::clone(name), *hash))
        .collect()
}

/// Find the single impl-method symbol containing the given fragment.
fn method_hash(module: &CompiledModule, fragment: &str) -> blake3::Hash {
    let matches: Vec<_> = module
        .function_names
        .iter()
        .filter(|(name, _)| name.contains("::") && name.contains(fragment))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one method symbol matching {fragment:?}, got {matches:?}"
    );
    *matches[0].1
}

const MONEY_MODULE: &str = r#"
    trait Show {
        fn show(self): number;
    }

    trait Scale {
        fn scale(self, factor: number): Self;
    }

    unique(11111111-1111-1111-1111-111111111111) type Money { cents: number }

    fn tax_rate(): number { 1.08 }

    impl Show for Money {
        fn show(self): number {
            self.cents * tax_rate()
        }
    }

    impl Scale for Money {
        fn scale(self, factor: number): Money {
            Money { cents: self.cents * factor }
        }
    }

    fn run(): number {
        let m = Money { cents: 100 };
        let scaled = m.scale(2);
        scaled.show()
    }
"#;

#[test]
fn same_source_compiles_to_same_hashes() {
    let a = compile(MONEY_MODULE);
    let b = compile(MONEY_MODULE);
    assert_eq!(named_hashes(&a), named_hashes(&b));
}

#[test]
fn trait_declaration_order_does_not_affect_hashes() {
    // Identical to MONEY_MODULE except the two trait declarations (and the
    // two impl blocks) are swapped. Declaration order must not leak into any
    // content hash.
    let reordered = r#"
        trait Scale {
            fn scale(self, factor: number): Self;
        }

        trait Show {
            fn show(self): number;
        }

        unique(11111111-1111-1111-1111-111111111111) type Money { cents: number }

        fn tax_rate(): number { 1.08 }

        impl Scale for Money {
            fn scale(self, factor: number): Money {
                Money { cents: self.cents * factor }
            }
        }

        impl Show for Money {
            fn show(self): number {
                self.cents * tax_rate()
            }
        }

        fn run(): number {
            let m = Money { cents: 100 };
            let scaled = m.scale(2);
            scaled.show()
        }
    "#;

    let original = compile(MONEY_MODULE);
    let swapped = compile(reordered);
    assert_eq!(named_hashes(&original), named_hashes(&swapped));
}

#[test]
fn unrelated_declarations_do_not_affect_method_hashes() {
    // Adding an unrelated trait, type, and function must not change the
    // hashes of existing impl methods.
    let extended = format!(
        r#"
        trait Unrelated {{
            fn noop(self): number;
        }}

        unique(22222222-2222-2222-2222-222222222222) type Other {{ x: number }}

        impl Unrelated for Other {{
            fn noop(self): number {{ self.x }}
        }}

        fn unused_helper(): number {{ 7 }}

        {MONEY_MODULE}
    "#
    );

    let original = compile(MONEY_MODULE);
    let with_extras = compile(&extended);

    assert_eq!(
        method_hash(&original, "Show::show"),
        method_hash(&with_extras, "Show::show"),
    );
    assert_eq!(
        method_hash(&original, "Scale::scale"),
        method_hash(&with_extras, "Scale::scale"),
    );
}

#[test]
fn method_body_change_changes_hash() {
    let modified = MONEY_MODULE.replace("self.cents * tax_rate()", "self.cents * tax_rate() + 1");
    assert_ne!(modified, MONEY_MODULE, "replacement must apply");

    let original = compile(MONEY_MODULE);
    let changed = compile(&modified);

    assert_ne!(
        method_hash(&original, "Show::show"),
        method_hash(&changed, "Show::show"),
        "editing a method body must change its content hash"
    );
    // The untouched method keeps its hash.
    assert_eq!(
        method_hash(&original, "Scale::scale"),
        method_hash(&changed, "Scale::scale"),
    );
}

#[test]
fn dependency_change_propagates_to_method_hash() {
    // `Show::show` calls `tax_rate`; changing `tax_rate`'s body must ripple
    // into the method's hash (hash = implementation + dependencies).
    let modified = MONEY_MODULE.replace(
        "fn tax_rate(): number { 1.08 }",
        "fn tax_rate(): number { 1.2 }",
    );
    assert_ne!(modified, MONEY_MODULE, "replacement must apply");

    let original = compile(MONEY_MODULE);
    let changed = compile(&modified);

    assert_ne!(
        method_hash(&original, "Show::show"),
        method_hash(&changed, "Show::show"),
        "changing a dependency must change the dependent method's hash"
    );
    // `Scale::scale` does not depend on tax_rate; its hash is stable.
    assert_eq!(
        method_hash(&original, "Scale::scale"),
        method_hash(&changed, "Scale::scale"),
    );
}

#[test]
fn impl_methods_are_stored_by_content_hash() {
    let module = compile(MONEY_MODULE);

    for (name, hash) in &module.function_names {
        if !name.contains("::") {
            continue;
        }
        let func = module
            .functions
            .get(hash)
            .unwrap_or_else(|| panic!("method {name} must be stored under its final hash"));
        assert_eq!(
            func.hash, *hash,
            "stored function's hash field must match its key"
        );

        // Every dependency of the method must itself resolve in the module
        // (no dangling temporary hashes).
        for dep in &func.dependencies {
            assert!(
                module.functions.contains_key(dep),
                "method {name} has unresolved dependency {dep}"
            );
        }
    }
}

#[test]
fn call_sites_reference_final_method_hashes() {
    // `run` calls both methods; its dependencies must be their final
    // content hashes, present in the module's function table.
    let module = compile(MONEY_MODULE);
    let run_hash = module
        .function_names
        .get("run")
        .expect("run function exists");
    let run_fn = module.functions.get(run_hash).expect("run is stored");

    let show = method_hash(&module, "Show::show");
    let scale = method_hash(&module, "Scale::scale");

    assert!(
        run_fn.dependencies.contains(&scale),
        "run must depend on Scale::scale's content hash"
    );
    // show is called on the result of scale; still a static dependency.
    assert!(
        run_fn.dependencies.contains(&show),
        "run must depend on Show::show's content hash"
    );
}
