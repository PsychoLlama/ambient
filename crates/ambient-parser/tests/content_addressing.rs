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

// ─────────────────────────────────────────────────────────────────────────────
// Canonical object invariants
// ─────────────────────────────────────────────────────────────────────────────

/// Every function's identity is the blake3 of its canonical object encoding,
/// and materializing the object reproduces the function byte-for-byte.
#[test]
fn every_function_is_materializable_from_a_self_verifying_object() {
    use ambient_engine::object::StoredObject;

    let module = compile(MONEY_MODULE);
    assert!(!module.objects.is_empty(), "module must carry objects");

    // Object keys are the blake3 of their encodings (redirects excepted:
    // they live at derived member hashes and are verified via their group).
    let mut materialized: HashMap<blake3::Hash, ambient_engine::bytecode::CompiledFunction> =
        HashMap::new();
    for (hash, object) in &module.objects {
        if matches!(object, StoredObject::Redirect { .. }) {
            continue;
        }
        assert_eq!(
            *hash,
            blake3::hash(&object.encode()),
            "object key must be the hash of its encoding"
        );
        for (func_hash, func) in object.materialize().expect("object materializes") {
            materialized.insert(func_hash, func);
        }
    }

    // Every compiled function must be reproducible from its object.
    for (hash, func) in &module.functions {
        let from_object = materialized
            .get(hash)
            .unwrap_or_else(|| panic!("function {hash} has no canonical object"));
        assert_eq!(from_object.bytecode, func.bytecode);
        assert_eq!(from_object.constants, func.constants);
        assert_eq!(from_object.local_count, func.local_count);
        assert_eq!(from_object.param_count, func.param_count);
        assert_eq!(from_object.dependencies, func.dependencies);
    }
}

const MUTUAL_RECURSION: &str = r"
    fn is_even(n: number): bool {
        if n == 0 { true } else { is_odd(n - 1) }
    }

    fn is_odd(n: number): bool {
        if n == 0 { false } else { is_even(n - 1) }
    }

    fn run(): bool {
        is_even(10)
    }
";

#[test]
fn mutually_recursive_functions_share_a_group_object() {
    use ambient_engine::object::StoredObject;

    let module = compile(MUTUAL_RECURSION);
    let names = named_hashes(&module);
    let even = names["is_even"];
    let odd = names["is_odd"];

    // Both members resolve to the same group via redirects.
    let group_of = |h: &blake3::Hash| match module.objects.get(h) {
        Some(StoredObject::Redirect { group, .. }) => *group,
        other => panic!("expected redirect at member hash, got {other:?}"),
    };
    assert_eq!(group_of(&even), group_of(&odd));

    // The group object exists and yields exactly these member hashes.
    let group = module
        .objects
        .get(&group_of(&even))
        .expect("group object stored");
    let members: Vec<blake3::Hash> = group
        .materialize()
        .expect("group materializes")
        .into_iter()
        .map(|(h, _)| h)
        .collect();
    assert!(members.contains(&even));
    assert!(members.contains(&odd));
}

#[test]
fn recursive_functions_survive_pack_roundtrip() {
    let module = compile(MUTUAL_RECURSION);
    let names = named_hashes(&module);

    let mut store = ambient_engine::store::Store::new();
    store.add_module(&module);

    let pack = store
        .extract_pack(&names["run"])
        .expect("run and its recursive deps must be shippable");
    let restored = ambient_engine::store::Store::deserialize(&pack).expect("pack decodes");

    for name in ["run", "is_even", "is_odd"] {
        assert!(
            restored.contains(&names[name]),
            "{name} must survive the pack roundtrip with its hash intact"
        );
    }
}

#[test]
fn recursive_group_hash_ignores_unrelated_lambdas() {
    // A recursive cycle that includes a lambda: `countdown` recurses through
    // a lambda, so the SCC is {countdown, <lambda>}. Lambda identity within
    // a group must come from canonical traversal order, not from
    // compilation-wide lambda counters — otherwise unrelated lambdas
    // elsewhere in the module would shift the hash.
    let base = r"
        fn countdown(n: number): number {
            let step = (k: number) => countdown(k);
            if n <= 0 { 0 } else { step(n - 1) }
        }

        fn run(): number { countdown(3) }
    ";
    let with_unrelated_lambda_first = r"
        fn unrelated(): number {
            let f = (x: number) => x * 2;
            f(21)
        }

        fn countdown(n: number): number {
            let step = (k: number) => countdown(k);
            if n <= 0 { 0 } else { step(n - 1) }
        }

        fn run(): number { countdown(3) }
    ";

    let a = compile(base);
    let b = compile(with_unrelated_lambda_first);
    assert_eq!(
        named_hashes(&a)["countdown"],
        named_hashes(&b)["countdown"],
        "an unrelated lambda earlier in the module must not change a recursive function's hash"
    );
}

#[test]
fn renaming_a_non_recursive_function_keeps_its_hash() {
    let a = compile("fn helper(): number { 41 + 1 }");
    let b = compile("fn renamed(): number { 41 + 1 }");
    assert_eq!(
        named_hashes(&a)["helper"],
        named_hashes(&b)["renamed"],
        "names of non-recursive functions must not affect their hashes"
    );
}

/// The disassembler's operand table must stay in sync with the compiler's
/// emission (and the VM's dispatch): disassembling real compiled code that
/// exercises jumps, calls, closures, handlers, and enums must decode every
/// instruction — a desynced table shows up as `??` or `<truncated>` lines.
#[test]
fn disassembler_decodes_all_compiled_instructions() {
    let source = r#"
        fn choose(n: number): number {
            if n > 3 { n * 2 } else { n + 1 }
        }

        fn is_even(n: number): bool {
            if n == 0 { true } else { is_odd(n - 1) }
        }

        fn is_odd(n: number): bool {
            if n == 0 { false } else { is_even(n - 1) }
        }

        fn apply(f: (number) -> number, x: number): number {
            f(x)
        }

        fn run(): number {
            let doubler = (x: number) => x * 2;
            let list = [1, 2, 3];
            let tuple = (1, "two");
            let record = { a: 1, b: "x" };
            apply(doubler, choose(5)) + record.a
        }
    "#;

    let module = compile(source);
    assert!(!module.functions.is_empty());
    for (hash, func) in &module.functions {
        let listing = ambient_engine::bytecode::disassemble(func);
        assert!(
            !listing.contains("??") && !listing.contains("<truncated>"),
            "disassembler failed to decode function {hash}:\n{listing}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inherent impls
//
// Inherent methods are ordinary functions under `<type-identity>::<method>`
// symbols (two segments — trait methods use three) and must uphold every
// content-addressing invariant trait methods do.
// ─────────────────────────────────────────────────────────────────────────────

const WALLET_MODULE: &str = r#"
    unique(33333333-3333-3333-3333-333333333333) type Wallet { cents: number }

    fn fee(): number { 3 }

    impl Wallet {
        fn charge(self): Wallet {
            Wallet { cents: self.cents - fee() }
        }
        fn empty(): Wallet {
            Wallet { cents: 0 }
        }
    }

    impl<T> Option<T> {
        fn or_default(self, fallback: T): T {
            match self {
                Some(v) => v,
                None => fallback,
            }
        }
    }

    fn run(): number {
        let w = Wallet { cents: 100 };
        let charged = w.charge();
        Some(charged.cents).or_default(Wallet::empty().cents)
    }
"#;

#[test]
fn inherent_same_source_compiles_to_same_hashes() {
    let a = compile(WALLET_MODULE);
    let b = compile(WALLET_MODULE);
    assert_eq!(named_hashes(&a), named_hashes(&b));
}

#[test]
fn inherent_method_symbols_use_type_identity() {
    // Nominal targets key their symbols by UUID; built-in constructors by
    // head name. Both are two-segment symbols, so they can never collide
    // with three-segment trait method symbols.
    let module = compile(WALLET_MODULE);
    let names: Vec<&str> = module
        .function_names
        .keys()
        .map(AsRef::as_ref)
        .filter(|n| n.contains("::"))
        .collect();

    assert!(
        names.contains(&"33333333-3333-3333-3333-333333333333::charge"),
        "expected uuid-keyed symbol, got {names:?}"
    );
    assert!(
        names.contains(&"33333333-3333-3333-3333-333333333333::empty"),
        "expected uuid-keyed symbol, got {names:?}"
    );
    assert!(
        names.contains(&"Option::or_default"),
        "expected head-name-keyed symbol, got {names:?}"
    );
}

#[test]
fn inherent_enum_method_keys_on_uuid() {
    // A declared enum is nominal: its inherent methods key on the enum's
    // uuid, exactly like a nominal `type`'s — never on its head name. This
    // is what makes an enum's methods robust against a same-named enum
    // elsewhere in a future multi-package world.
    let module = compile(
        r#"
        unique(44444444-4444-4444-4444-444444444444) enum Toggle { On, Off }

        impl Toggle {
            fn flipped(self): Toggle {
                match self {
                    On => Off,
                    Off => On,
                }
            }
        }

        fn run(): Toggle {
            On.flipped()
        }
    "#,
    );
    let names: Vec<&str> = module
        .function_names
        .keys()
        .map(AsRef::as_ref)
        .filter(|n: &&str| n.contains("::"))
        .collect();

    assert!(
        names.contains(&"44444444-4444-4444-4444-444444444444::flipped"),
        "expected uuid-keyed enum method symbol, got {names:?}"
    );
}

#[test]
fn inherent_impl_block_order_does_not_affect_hashes() {
    // Splitting methods across impl blocks and reordering them (and the
    // impl blocks themselves) must not change any hash.
    let reordered = r#"
        unique(33333333-3333-3333-3333-333333333333) type Wallet { cents: number }

        impl<T> Option<T> {
            fn or_default(self, fallback: T): T {
                match self {
                    Some(v) => v,
                    None => fallback,
                }
            }
        }

        impl Wallet {
            fn empty(): Wallet {
                Wallet { cents: 0 }
            }
        }

        impl Wallet {
            fn charge(self): Wallet {
                Wallet { cents: self.cents - fee() }
            }
        }

        fn fee(): number { 3 }

        fn run(): number {
            let w = Wallet { cents: 100 };
            let charged = w.charge();
            Some(charged.cents).or_default(Wallet::empty().cents)
        }
    "#;

    let original = compile(WALLET_MODULE);
    let swapped = compile(reordered);
    assert_eq!(named_hashes(&original), named_hashes(&swapped));
}

#[test]
fn inherent_dependency_change_propagates_to_method_hash() {
    // `charge` calls `fee`; changing `fee` must ripple into `charge`'s
    // hash but leave `empty` and `or_default` untouched.
    let modified = WALLET_MODULE.replace("fn fee(): number { 3 }", "fn fee(): number { 5 }");
    assert_ne!(modified, WALLET_MODULE, "replacement must apply");

    let original = compile(WALLET_MODULE);
    let changed = compile(&modified);

    assert_ne!(
        method_hash(&original, "charge"),
        method_hash(&changed, "charge"),
        "changing a dependency must change the dependent method's hash"
    );
    assert_eq!(
        method_hash(&original, "empty"),
        method_hash(&changed, "empty"),
    );
    assert_eq!(
        method_hash(&original, "or_default"),
        method_hash(&changed, "or_default"),
    );
}

#[test]
fn inherent_call_sites_reference_final_method_hashes() {
    // `run` dispatches to all three methods; its dependencies must be
    // their final content hashes.
    let module = compile(WALLET_MODULE);
    let run_hash = module
        .function_names
        .get("run")
        .expect("run function exists");
    let run_fn = module.functions.get(run_hash).expect("run is stored");

    for fragment in ["charge", "empty", "or_default"] {
        let hash = method_hash(&module, fragment);
        assert!(
            run_fn.dependencies.contains(&hash),
            "run must depend on {fragment}'s content hash"
        );
        assert!(
            module.functions.contains_key(&hash),
            "{fragment} must be stored under its final hash"
        );
    }
}
