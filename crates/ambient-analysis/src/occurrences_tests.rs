//! Occurrence-collector unit tests, split out of `occurrences.rs` to keep it
//! under the per-file line budget (see `session_tests.rs` for the pattern).

use super::*;

/// Collect occurrences for a single source as the package root.
fn occurrences_of(source: &str) -> Vec<Occurrence> {
    let recovered = ambient_parser::parse_recovering(source);
    let module_path = ModulePath::root();
    let mut registry = crate::core_platform_registry();
    registry.register(&module_path, std::sync::Arc::new(recovered.module.clone()));
    collect_occurrences(&recovered.module, &module_path, &registry)
}

/// Collect occurrences against the **checked** module — the production input
/// for methods, whose dispatch symbols the checker fills.
fn checked_occurrences_of(source: &str) -> Vec<Occurrence> {
    let recovered = ambient_parser::parse_recovering(source);
    let module_path = ModulePath::root();
    let mut registry = crate::core_platform_registry();
    registry.register(&module_path, std::sync::Arc::new(recovered.module.clone()));
    let checked = ambient_engine::infer::check_module_with_registry(
        recovered.module,
        &module_path,
        &registry,
    )
    .module;
    collect_occurrences(&checked, &module_path, &registry)
}

/// Every `Method`-target occurrence for `name`, split into (defs, refs).
fn methods<'a>(occ: &'a [Occurrence], name: &str) -> (Vec<&'a Occurrence>, Vec<&'a Occurrence>) {
    let is_method = |o: &&Occurrence| {
        matches!(o.target, SymbolTarget::Method { .. }) && o.target.name().as_ref() == name
    };
    (
        occ.iter()
            .filter(|o| is_method(o) && o.is_definition)
            .collect(),
        occ.iter()
            .filter(|o| is_method(o) && !o.is_definition)
            .collect(),
    )
}

const UU: &str = "A1B2C3D4-0000-0000-0000-0000000000";

#[test]
fn trait_method_call_and_impl_declaration_share_a_method_target() {
    let src = format!(
        "unique({UU}01) trait Show {{ fn show(self): Number; }}\n\
             unique({UU}02) struct Foo {{ x: Number }}\n\
             impl Show for Foo {{ fn show(self): Number {{ self.x }} }}\n\
             fn run(f: Foo): Number {{ f.show() }}"
    );
    let occ = checked_occurrences_of(&src);
    let (defs, refs) = methods(&occ, "show");
    assert_eq!(defs.len(), 1, "the impl method declaration");
    assert_eq!(refs.len(), 1, "the `f.show()` call");
    assert_eq!(defs[0].target, refs[0].target);
}

#[test]
fn inherent_method_call_resolves_to_its_declaration() {
    let src = format!(
        "unique({UU}03) struct Foo {{ x: Number }}\n\
             impl Foo {{ fn dbl(self): Number {{ self.x * 2 }} }}\n\
             fn run(f: Foo): Number {{ f.dbl() }}"
    );
    let occ = checked_occurrences_of(&src);
    let (defs, refs) = methods(&occ, "dbl");
    assert_eq!(defs.len(), 1);
    assert_eq!(refs.len(), 1);
    assert_eq!(defs[0].target, refs[0].target);
}

#[test]
fn associated_call_resolves_to_its_declaration() {
    let src = format!(
        "unique({UU}04) struct Foo {{ x: Number }}\n\
             impl Foo {{ fn make(): Foo {{ Foo {{ x: 1 }} }} }}\n\
             fn run(): Foo {{ Foo::make() }}"
    );
    let occ = checked_occurrences_of(&src);
    let (defs, refs) = methods(&occ, "make");
    assert_eq!(defs.len(), 1, "the associated impl method");
    assert_eq!(refs.len(), 1, "the `Foo::make()` call");
    assert_eq!(defs[0].target, refs[0].target);
}

#[test]
fn ability_perform_and_handler_arm_resolve_to_the_declaration() {
    let src = format!(
        "unique({UU}05) ability Counter {{ fn tick(): Number {{ 0 }} }}\n\
             fn run(): Number {{ with {{ Counter::tick() => resume(0) }} handle {{ Counter::tick!() }} }}"
    );
    let occ = checked_occurrences_of(&src);
    let (defs, refs) = methods(&occ, "tick");
    assert_eq!(defs.len(), 1, "the ability method declaration");
    assert_eq!(refs.len(), 2, "the perform and the handler arm");
    for r in &refs {
        assert_eq!(r.target, defs[0].target);
    }
}

#[test]
fn parsed_input_emits_no_method_occurrences() {
    // The parse-only walk has no dispatch symbols, so methods stay unindexed
    // (single-file unit tests below rely on this).
    let src = format!(
        "unique({UU}06) struct Foo {{ x: Number }}\n\
             impl Foo {{ fn dbl(self): Number {{ self.x }} }}\n\
             fn run(f: Foo): Number {{ f.dbl() }}"
    );
    let occ = occurrences_of(&src);
    assert!(
        !occ.iter()
            .any(|o| matches!(o.target, SymbolTarget::Method { .. })),
        "parsed input must not synthesize method occurrences"
    );
}

fn find<'a>(occ: &'a [Occurrence], name: &str, is_def: bool) -> Vec<&'a Occurrence> {
    occ.iter()
        .filter(|o| o.target.name().as_ref() == name && o.is_definition == is_def)
        .collect()
}

#[test]
fn function_definition_and_call_share_a_target() {
    let occ = occurrences_of("fn helper(): Number { 1 }\nfn run(): Number { helper() }");
    let def = find(&occ, "helper", true);
    let refs = find(&occ, "helper", false);
    assert_eq!(def.len(), 1);
    assert_eq!(refs.len(), 1);
    assert_eq!(def[0].target, refs[0].target);
}

#[test]
fn local_param_and_uses_share_a_target() {
    let occ = occurrences_of("fn run(x: Number): Number { x + x }");
    let all: Vec<_> = occ
        .iter()
        .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
        .collect();
    assert_eq!(all.len(), 3, "one param def + two uses");
    assert_eq!(all.iter().filter(|o| o.is_definition).count(), 1);
    for o in &all {
        assert_eq!(o.target, all[0].target);
    }
}

#[test]
fn param_definition_span_excludes_the_type() {
    let occ = occurrences_of("fn run(count: Number): Number { count }");
    let def = find(&occ, "count", true);
    assert_eq!(def.len(), 1);
    // "fn run(" is 7 bytes; `count` spans [7, 12), not `count: number`.
    assert_eq!(def[0].span, Span::new(7, 12));
}

#[test]
fn let_shadow_rhs_refers_to_outer_binding() {
    // `let y = x; ...` — the two `x` uses are the param; `y` is distinct.
    let occ = occurrences_of("fn run(x: Number): Number { let y = x; y }");
    let xs: Vec<_> = occ
        .iter()
        .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
        .collect();
    assert_eq!(xs.len(), 2, "param def + one use in the initializer");
    let ys: Vec<_> = occ
        .iter()
        .filter(|o| o.target.is_local() && o.target.name().as_ref() == "y")
        .collect();
    assert_eq!(ys.len(), 2, "let def + one use in the result");
    assert_ne!(xs[0].target, ys[0].target);
}

#[test]
fn item_identity_is_span_independent() {
    // The whole point of Fqn keying: shifting a definition's span (here by
    // leading blank lines) must not change its target identity, so another
    // module's reference — built at a different revision — still matches.
    let a = occurrences_of("fn helper(): Number { 1 }\nfn run(): Number { helper() }");
    let b = occurrences_of("\n\nfn helper(): Number { 1 }\nfn run(): Number { helper() }");
    let da = find(&a, "helper", true);
    let db = find(&b, "helper", true);
    assert_eq!(da.len(), 1);
    assert_eq!(db.len(), 1);
    // The definition spans differ (b is shifted by two newlines)...
    assert_ne!(da[0].span, db[0].span);
    // ...but the identities are equal, because they key on the Fqn.
    assert_eq!(da[0].target, db[0].target);
    // And a reference in b still collapses onto that identity.
    let rb = find(&b, "helper", false);
    assert_eq!(rb.len(), 1);
    assert_eq!(rb[0].target, da[0].target);
}

#[test]
fn variant_construction_and_declaration_share_a_target() {
    let occ = occurrences_of(
        "unique(A1B2C3D4-0000-0000-0000-0000000000A1) enum Shape { Circle(Number), Square }\n\
             fn run(): Shape { Shape::Circle(2.0) }",
    );
    let def = find(&occ, "Circle", true);
    let refs = find(&occ, "Circle", false);
    assert_eq!(def.len(), 1, "one variant declaration");
    assert_eq!(refs.len(), 1, "one construction site");
    assert_eq!(def[0].target, refs[0].target);
    // The variant identity is distinct from the enum's.
    let enum_def = find(&occ, "Shape", true);
    assert_eq!(enum_def.len(), 1);
    assert_ne!(enum_def[0].target, def[0].target);
}

#[test]
fn bare_and_qualified_variant_spellings_collapse() {
    // The same variant referenced bare (imported-style, here same-module)
    // and via `Enum::Variant` must land on one identity.
    let occ = occurrences_of(
        "unique(A1B2C3D4-0000-0000-0000-0000000000A2) enum Dir { Up, Down }\n\
             fn a(): Dir { Up }\n\
             fn b(): Dir { Dir::Up }",
    );
    let def = find(&occ, "Up", true);
    let refs = find(&occ, "Up", false);
    assert_eq!(def.len(), 1);
    assert_eq!(refs.len(), 2, "bare `Up` and `Dir::Up`");
    for r in &refs {
        assert_eq!(r.target, def[0].target);
    }
}

#[test]
fn variant_pattern_references_the_variant() {
    let occ = occurrences_of(
        "unique(A1B2C3D4-0000-0000-0000-0000000000A3) enum Opt { Has(Number), Empty }\n\
             fn run(x: Opt): Number { match x { Has(n) => n, Empty => 0 } }",
    );
    let has_def = find(&occ, "Has", true);
    let has_refs = find(&occ, "Has", false);
    assert_eq!(has_def.len(), 1);
    assert_eq!(has_refs.len(), 1, "the `Has(n)` pattern");
    assert_eq!(has_def[0].target, has_refs[0].target);
    // `n` bound by the pattern is a local, not the variant.
    let empty_refs = find(&occ, "Empty", false);
    assert_eq!(empty_refs.len(), 1, "the `Empty` pattern");
}

#[test]
fn inner_shadow_is_a_distinct_binding() {
    // The lambda's `x` shadows the param `x`: two distinct targets.
    let occ = occurrences_of("fn run(x: Number): Number { let f = (x) => x + 1; x }");
    let targets: std::collections::HashSet<_> = occ
        .iter()
        .filter(|o| o.target.is_local() && o.target.name().as_ref() == "x")
        .map(|o| match &o.target {
            SymbolTarget::Local { binding_id, .. } => *binding_id,
            SymbolTarget::Item { .. } | SymbolTarget::Method { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(
        targets.len(),
        2,
        "param x and lambda x are different bindings"
    );
}
