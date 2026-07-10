//! Structured build diagnostics.
//!
//! `build_package` must surface type errors as spanned, structured errors
//! (not flattened strings) so the compiling commands (`ambient
//! run`/`compile`/`dev`) can render them with source context — byte-for-byte
//! what `ambient check` prints. These tests pin the engine-level contract
//! that makes that possible.

use std::fs;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildError, ParseFailure, build_package};
use tempfile::TempDir;

/// Parse for the build pipeline, mapping a parse error to the engine's
/// renderable `ParseFailure` (mirrors the CLI's `parse_source`).
fn parse_source(source: &str) -> Result<Module, ParseFailure> {
    ambient_parser::parse(source).map_err(|e| ParseFailure {
        message: e.kind.to_string(),
        span: (e.span.start, e.span.end),
        context: e.context,
    })
}

/// Write a single-module package with `main.ab` = `source`.
fn temp_package(source: &str) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"diag\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    fs::create_dir_all(&src).expect("create src");
    fs::write(src.join("main.ab"), source).expect("write main.ab");
    dir
}

#[test]
fn build_package_type_error_is_structured_with_nonzero_span() {
    // `run` returns a number from a function declared to return a string.
    let dir = temp_package("pub fn run(): String { 42 }\n");

    let stubs = ambient_platform::stub_natives();
    let Err(err) = build_package(
        dir.path(),
        parse_source,
        &ambient_engine::build::BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    ) else {
        panic!("type error should fail the build");
    };

    let BuildError::TypeCheck {
        source,
        errors,
        path,
        ..
    } = err
    else {
        panic!("expected a structured TypeCheck error, got: {err:?}");
    };

    // The source and file path travel with the error so the frontend can
    // render source context.
    assert!(source.contains("pub fn run"));
    assert!(path.ends_with("main.ab"));

    // Structured, spanned errors — not flattened strings.
    assert!(!errors.is_empty(), "expected at least one type error");
    let error = &errors[0];
    let (start, end) = error.span;
    assert!(
        end > start,
        "type error must carry a nonzero span, got ({start}, {end})"
    );
    // The span points into the source (the offending `42`).
    assert!(
        (end as usize) <= source.len(),
        "span end {end} out of bounds for source of len {}",
        source.len()
    );
}

#[test]
fn build_package_parse_error_is_structured_with_span() {
    let dir = temp_package("pub fn run(): Number {\n  1 +\n}\n");

    let stubs = ambient_platform::stub_natives();
    let Err(err) = build_package(
        dir.path(),
        parse_source,
        &ambient_engine::build::BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    ) else {
        panic!("parse error should fail the build");
    };

    let BuildError::Parse {
        source,
        error,
        path,
        ..
    } = err
    else {
        panic!("expected a structured Parse error, got: {err:?}");
    };

    assert!(source.contains("pub fn run"));
    assert!(path.ends_with("main.ab"));
    let (start, end) = error.span;
    assert!(end >= start, "parse error span must be well-formed");
    assert!((end as usize) <= source.len());
}
