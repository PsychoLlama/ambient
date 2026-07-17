//! End-to-end coverage of module interfaces (incremental-compilation
//! Phase 1): byte-stability across rebuilds, insensitivity to private
//! items / formatting / declaration order / public non-impl bodies, and
//! per-channel sensitivity — one flip per cross-module channel.
//!
//! These drive the real pipeline: parse `.ab` source, `build_package`, and
//! read `BuildResult::interfaces` / `dispatch_surface_hash`.

use std::fs;

use ambient_engine::build::{BuildOptions, BuildResult, build_package};
use tempfile::TempDir;

mod common;
use common::parse_source;

/// Build a package from `(relative path, source)` files under `src/`.
fn build(files: &[(&str, &str)]) -> BuildResult {
    let dir = TempDir::new().expect("temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"test_pkg\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("src");
    for (path, source) in files {
        let file = src.join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(&file, source).expect("write");
    }
    build_package(dir.path(), parse_source, &BuildOptions::default()).expect("build succeeds")
}

/// The single-module package's `main` interface hash.
fn main_iface_hash(source: &str) -> blake3::Hash {
    build(&[("main.ab", source)]).interfaces[MAIN].interface_hash
}

fn main_ast_hash(source: &str) -> blake3::Hash {
    build(&[("main.ab", source)]).interfaces[MAIN].resolved_ast_hash
}

const MAIN: &str = "workspace::test_pkg";

// ─────────────────────────────────────────────────────────────────────────────
// Byte-stability
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn building_twice_yields_identical_encodings_and_hashes() {
    let source = "pub fn add(x: Number, y: Number): Number { x + y }\n\
                  unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n";
    let a = build(&[("main.ab", source)]);
    let b = build(&[("main.ab", source)]);

    // Every module (core, platform, user) matches byte-for-byte.
    assert_eq!(a.interfaces.len(), b.interfaces.len());
    for (key, sa) in &a.interfaces {
        let sb = b.interfaces.get(key).expect("same module set");
        assert_eq!(
            sa.interface.encode(),
            sb.interface.encode(),
            "encoding differs for {key}"
        );
        assert_eq!(
            sa.interface_hash, sb.interface_hash,
            "hash differs for {key}"
        );
        assert_eq!(sa.resolved_ast_hash, sb.resolved_ast_hash);
    }
    assert_eq!(a.dispatch_surface_hash, b.dispatch_surface_hash);
}

// ─────────────────────────────────────────────────────────────────────────────
// Insensitivity
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn private_items_do_not_change_the_interface() {
    let base = "pub fn f(x: Number): Number { x }\n";
    let with_private = "pub fn f(x: Number): Number { x }\n\
                        fn secret(): Number { 99 }\n\
                        const HIDDEN: Number = 7;\n";
    assert_eq!(main_iface_hash(base), main_iface_hash(with_private));
}

#[test]
fn private_function_body_does_not_change_the_interface() {
    let a = "pub fn f(): Number { helper() }\nfn helper(): Number { 1 }\n";
    let b = "pub fn f(): Number { helper() }\nfn helper(): Number { 2 }\n";
    assert_eq!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn public_non_impl_body_does_not_change_the_interface() {
    let a = "pub fn f(x: Number): Number { x + 1 }\n";
    let b = "pub fn f(x: Number): Number { x + 2 }\n";
    assert_eq!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn formatting_and_comments_do_not_change_the_interface() {
    let a = "pub fn f(x: Number): Number { x }\n";
    let b = "// a comment\n\
             pub fn f(x:    Number)  :  Number {\n    x   // trailing\n}\n";
    assert_eq!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn declaration_order_does_not_change_the_interface() {
    let a = "pub fn a(): Number { 1 }\npub fn b(): Number { 2 }\n";
    let b = "pub fn b(): Number { 2 }\npub fn a(): Number { 1 }\n";
    assert_eq!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn resolved_ast_hash_is_insensitive_to_formatting() {
    let a = "pub fn f(x: Number): Number { x }\nfn g(): Number { 1 }\n";
    let b = "pub  fn  f(x: Number): Number {  x  }\n\n// note\nfn g(): Number { 1 }\n";
    assert_eq!(main_ast_hash(a), main_ast_hash(b));
}

// ─────────────────────────────────────────────────────────────────────────────
// Sensitivity — one flip per channel
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn pub_signature_edit_flips_the_hash() {
    // Bodies return a literal so both variants type-check; only the
    // parameter type differs.
    let a = "pub fn f(x: Number): Number { 0 }\n";
    let b = "pub fn f(x: String): Number { 0 }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn struct_field_add_flips_the_hash() {
    let a = "pub unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number }\n";
    let b = "pub unique(A1B2C3D4-0000-0000-0000-000000000001) struct P { x: Number, y: Number }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn enum_variant_rename_flips_the_hash() {
    let a = "pub unique(E1B2C3D4-0000-0000-0000-000000000001) enum E { A, B(Number) }\n";
    let b = "pub unique(E1B2C3D4-0000-0000-0000-000000000001) enum E { A, C(Number) }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn trait_method_signature_flips_the_hash() {
    let a = "pub unique(F1B2C3D4-0000-0000-0000-000000000001) trait T { fn m(self): Number; }\n";
    let b = "pub unique(F1B2C3D4-0000-0000-0000-000000000001) trait T { fn m(self): String; }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn impl_method_body_edit_flips_the_hash() {
    let prelude = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n";
    let a = format!(
        "{prelude}impl Add for V {{ fn add(self, o: V): V {{ V {{ x: self.x + o.x }} }} }}\n"
    );
    let b = format!(
        "{prelude}impl Add for V {{ fn add(self, o: V): V {{ V {{ x: self.x - o.x }} }} }}\n"
    );
    assert_ne!(main_iface_hash(&a), main_iface_hash(&b));
}

#[test]
fn ability_default_body_edit_flips_the_hash() {
    let a = "pub unique(B1B2C3D4-0000-0000-0000-000000000001) ability Counter { \
             fn tick(): Number { 1 } }\n";
    let b = "pub unique(B1B2C3D4-0000-0000-0000-000000000001) ability Counter { \
             fn tick(): Number { 2 } }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn const_value_change_flips_the_hash() {
    let a = "pub const K: Number = 1;\n";
    let b = "pub const K: Number = 2;\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn pubness_toggle_flips_the_hash() {
    let a = "pub fn f(): Number { 1 }\n";
    let b = "fn f(): Number { 1 }\n";
    assert_ne!(main_iface_hash(a), main_iface_hash(b));
}

#[test]
fn reexport_retarget_flips_the_hash() {
    let with_target = |target: &str| {
        [
            ("main.ab", format!("pub use pkg::{target}::foo;\n")),
            ("a.ab", "pub fn foo(): Number { 1 }\n".to_string()),
            ("b.ab", "pub fn foo(): Number { 2 }\n".to_string()),
        ]
    };
    let a = with_target("a");
    let b = with_target("b");
    let a_refs: Vec<(&str, &str)> = a.iter().map(|(p, s)| (*p, s.as_str())).collect();
    let b_refs: Vec<(&str, &str)> = b.iter().map(|(p, s)| (*p, s.as_str())).collect();
    let ha = build(&a_refs).interfaces[MAIN].interface_hash;
    let hb = build(&b_refs).interfaces[MAIN].interface_hash;
    assert_ne!(ha, hb);
}

#[test]
fn resolved_ast_hash_flips_on_private_body_change() {
    let a = "pub fn f(): Number { g() }\nfn g(): Number { 1 }\n";
    let b = "pub fn f(): Number { g() }\nfn g(): Number { 2 }\n";
    assert_ne!(main_ast_hash(a), main_ast_hash(b));
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch surface
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn dispatch_surface_hash_reacts_to_impl_and_ability_changes() {
    let base = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n\
                impl Add for V { fn add(self, o: V): V { V { x: self.x + o.x } } }\n\
                impl V { fn mag(self): Number { self.x } }\n";
    let base_hash = build(&[("main.ab", base)]).dispatch_surface_hash;

    // Phase 5 step 2: the dispatch surface is *body-free*. Editing an impl
    // method body no longer moves it — body-driven callee-hash moves are
    // covered by link validation (build) and the dependency channel
    // (importers), never by this build-global fold.
    let edited_body = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n\
                       impl Add for V { fn add(self, o: V): V { V { x: self.x - o.x } } }\n\
                       impl V { fn mag(self): Number { self.x + 1 } }\n";
    assert_eq!(
        base_hash,
        build(&[("main.ab", edited_body)]).dispatch_surface_hash,
        "an impl body edit must not move the body-free dispatch surface"
    );

    // A method *signature* change does move it (it alters every dispatcher's
    // inference and can shift coherence).
    let edited_sig = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n\
                      impl Add for V { fn add(self, o: V): V { V { x: self.x + o.x } } }\n\
                      impl V { fn mag(self, k: Number): Number { self.x + k } }\n";
    assert_ne!(
        base_hash,
        build(&[("main.ab", edited_sig)]).dispatch_surface_hash,
        "an impl-method signature change must move the dispatch surface"
    );

    // Adding an impl (a new `(trait, type)` / inherent target) moves it —
    // this is the coherence channel that must stay build-global.
    let added_impl = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n\
                      impl Add for V { fn add(self, o: V): V { V { x: self.x + o.x } } }\n\
                      impl V { fn mag(self): Number { self.x } }\n\
                      impl Sub for V { fn sub(self, o: V): V { V { x: self.x - o.x } } }\n";
    assert_ne!(
        base_hash,
        build(&[("main.ab", added_impl)]).dispatch_surface_hash,
        "adding an impl must move the coherence surface"
    );

    // A change touching neither impls nor abilities leaves it stable.
    let extra_pub_fn = "unique(A1B2C3D4-0000-0000-0000-000000000001) struct V { x: Number }\n\
                        impl Add for V { fn add(self, o: V): V { V { x: self.x + o.x } } }\n\
                        impl V { fn mag(self): Number { self.x } }\n\
                        pub fn unrelated(): Number { 5 }\n";
    assert_eq!(
        base_hash,
        build(&[("main.ab", extra_pub_fn)]).dispatch_surface_hash
    );
}
