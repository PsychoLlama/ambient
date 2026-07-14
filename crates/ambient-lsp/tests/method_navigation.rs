//! Navigation (goto-definition, hover, find-references) for method-shaped
//! references: trait method calls, inherent methods, associated calls, and
//! ability methods at perform sites and handler arms.
//!
//! These are served from the occurrence index, which keys methods on the
//! engine's content-addressed dispatch symbol — so a call site and the
//! declaration it dispatches to collapse to one identity. See
//! `ambient_analysis::occurrences::SymbolTarget::Method`.

use ambient_lsp::test_harness::{LspTest, TestClient};

/// UUID prefix for the structs/traits/abilities these fixtures declare.
const U: &str = "A1B2C3D4-0000-0000-0000-0000000000";

// ─────────────────────────────────────────────────────────────────────────
// Goto-definition (single file)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn goto_trait_method_call_lands_on_the_impl_method() {
    let src = format!(
        "unique({U}01) trait Show {{ fn show(self): Number; }}\n\
         unique({U}02) struct Foo {{ x: Number }}\n\
         impl Show for Foo {{ fn show(self): Number {{ self.x }} }}\n\
         fn run(f: Foo): Number {{ f.show/*use*/() }}"
    );
    let (test, locations) = LspTest::new()
        .with_source(&src)
        .goto_definition_at("use")
        .raw();
    assert!(!locations.is_empty(), "expected the impl method definition");
    // The impl (with `show`) is on line 2 (0-indexed).
    assert_eq!(locations[0].range.start.line, 2);
    test.shutdown();
}

#[test]
fn goto_inherent_method_call_lands_on_the_declaration() {
    let src = format!(
        "unique({U}03) struct Foo {{ x: Number }}\n\
         impl Foo {{ fn dbl(self): Number {{ self.x * 2 }} }}\n\
         fn run(f: Foo): Number {{ f.dbl/*use*/() }}"
    );
    let (test, locations) = LspTest::new()
        .with_source(&src)
        .goto_definition_at("use")
        .raw();
    assert!(!locations.is_empty(), "expected the inherent method");
    assert_eq!(locations[0].range.start.line, 1);
    test.shutdown();
}

#[test]
fn goto_associated_call_lands_on_the_declaration() {
    let src = format!(
        "unique({U}04) struct Foo {{ x: Number }}\n\
         impl Foo {{ fn make(): Foo {{ Foo {{ x: 1 }} }} }}\n\
         fn run(): Foo {{ Foo::make/*use*/() }}"
    );
    let (test, locations) = LspTest::new()
        .with_source(&src)
        .goto_definition_at("use")
        .raw();
    assert!(!locations.is_empty(), "expected the associated method");
    assert_eq!(locations[0].range.start.line, 1);
    test.shutdown();
}

#[test]
fn goto_ability_perform_lands_on_the_method_declaration() {
    let src = format!(
        "unique({U}05) ability Counter {{ fn tick(): Number {{ 0 }} }}\n\
         fn run(): Number {{ with {{ Counter::tick() => resume(0) }} handle {{ Counter::tick/*use*/!() }} }}"
    );
    let (test, locations) = LspTest::new()
        .with_source(&src)
        .goto_definition_at("use")
        .raw();
    assert!(!locations.is_empty(), "expected the ability method decl");
    assert_eq!(locations[0].range.start.line, 0);
    test.shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// Goto-definition (cross-module)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn goto_trait_method_call_across_modules() {
    let shapes = format!(
        "pub unique({U}06) trait Area {{ fn area(self): Number; }}\n\
         pub unique({U}07) struct Circle {{ r: Number }}\n\
         impl Area for Circle {{ fn area(self): Number {{ self.r }} }}\n"
    );
    let main = "use pkg::shapes::{Area, Circle};\nfn run(c: Circle): Number { c.area/*use*/() }\n";
    LspTest::new()
        .with_package()
        .with_file("src/shapes.ab", &shapes)
        .with_file("src/main.ab", main)
        .open_file("src/main.ab")
        .goto_definition_at("use")
        .expect_file("shapes.ab")
        .done()
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// Hover
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn hover_on_trait_method_call_shows_the_signature() {
    let src = format!(
        "unique({U}08) trait Show {{ fn show(self): Number; }}\n\
         unique({U}09) struct Foo {{ x: Number }}\n\
         impl Show for Foo {{ fn show(self): Number {{ self.x }} }}\n\
         fn run(f: Foo): Number {{ f.show/*h*/() }}"
    );
    LspTest::new()
        .with_source(&src)
        .hover_at("h")
        .expect_contains("fn show(self): Number")
        .shutdown();
}

#[test]
fn hover_on_ability_perform_shows_the_signature() {
    let src = format!(
        "unique({U}0A) ability Counter {{ fn tick(): Number {{ 0 }} }}\n\
         fn run(): Number {{ with {{ Counter::tick() => resume(0) }} handle {{ Counter::tick/*h*/!() }} }}"
    );
    LspTest::new()
        .with_source(&src)
        .hover_at("h")
        .expect_contains("fn tick(): Number")
        .shutdown();
}

// ─────────────────────────────────────────────────────────────────────────
// Find-references
// ─────────────────────────────────────────────────────────────────────────

/// 0-indexed (line, character) of the `occurrence`-th (0-based) `needle`.
fn pos_of(text: &str, needle: &str, occurrence: usize) -> (u32, u32) {
    let mut from = 0;
    let mut byte = 0;
    for _ in 0..=occurrence {
        let idx = text[from..].find(needle).expect("needle") + from;
        byte = idx;
        from = idx + needle.len();
    }
    let line = text[..byte].matches('\n').count() as u32;
    let col = byte - text[..byte].rfind('\n').map_or(0, |i| i + 1);
    (line, col as u32)
}

#[test]
fn references_on_trait_method_find_call_and_declaration() {
    let src = format!(
        "unique({U}0B) trait Show {{ fn show(self): Number; }}\n\
         unique({U}0C) struct Foo {{ x: Number }}\n\
         impl Show for Foo {{ fn show(self): Number {{ self.x }} }}\n\
         fn run(f: Foo): Number {{ f.show() }}\n"
    );
    let mut client = TestClient::new();
    let uri = "inmemory:///test.ab".parse().expect("uri");
    client.open_document(uri, &src);
    let uri = "inmemory:///test.ab".parse().expect("uri");

    // The `show` at the call site (occurrence 2: trait decl, impl decl, call).
    let (line, ch) = pos_of(&src, "show", 2);
    let refs = client.references(&uri, line, ch, true);

    // The impl declaration and the call site both resolve to the method — two
    // occurrences on the two lines (the type-blind trait decl is not indexed).
    let lines: Vec<u32> = refs.iter().map(|l| l.range.start.line).collect();
    assert!(lines.contains(&2), "impl method declaration: {lines:?}");
    assert!(lines.contains(&3), "call site: {lines:?}");
    client.shutdown();
}
