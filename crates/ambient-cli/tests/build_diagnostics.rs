//! Structured build diagnostics.
//!
//! `build_package` must surface type errors as spanned, structured errors
//! (not flattened strings) so the compiling commands (`ambient
//! run`/`compile`/`dev`) can render them with source context — byte-for-byte
//! what `ambient check` prints. These tests pin the engine-level contract
//! that makes that possible.

use std::fs;

use ambient_engine::ast::Module;
use ambient_engine::build::{BuildError, BuildOptions, ParseFailure, build_package};
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

/// Write a package from `(src-relative path, source)` pairs.
fn temp_multi_package(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join("ambient.toml"),
        "[package]\nname = \"diag\"\nversion = \"0.1.0\"\n\n[build]\nsrc = \"src\"\n",
    )
    .expect("write manifest");
    let src = dir.path().join("src");
    for (rel, body) in files {
        let path = src.join(rel);
        fs::create_dir_all(path.parent().unwrap()).expect("create src dir");
        fs::write(path, body).expect("write module");
    }
    dir
}

/// The check pre-pass runs before the compile walk, so a cold build surfaces the
/// type errors of *every* failing module at once — not just the first in compile
/// order. Two independent modules each fail here; both must come back, in the
/// pre-pass's deterministic (by module-identity) order.
#[test]
fn cold_build_reports_type_errors_in_every_failing_module() {
    let dir = temp_multi_package(&[
        ("main.ab", "pub fn run(): Number { 0 }\n"),
        // Each returns a number from a `String`-typed function: a type error.
        ("alpha.ab", "pub fn a(): String { 1 }\n"),
        ("beta.ab", "pub fn b(): String { 2 }\n"),
    ]);

    let stubs = ambient_platform::stub_natives();
    let Err(err) = build_package(
        dir.path(),
        parse_source,
        &BuildOptions {
            platform_modules: ambient_platform::platform_modules(),
            natives: Some(&stubs),
            ..Default::default()
        },
    ) else {
        panic!("type errors should fail the build");
    };

    let BuildError::TypeCheck { failures } = err else {
        panic!("expected a structured TypeCheck error, got: {err:?}");
    };

    assert_eq!(
        failures.len(),
        2,
        "both failing modules must surface together, not just the first"
    );
    // Deterministically ordered by module identity.
    let modules: Vec<&str> = failures.iter().map(|f| f.module.as_str()).collect();
    let mut sorted = modules.clone();
    sorted.sort_unstable();
    assert_eq!(
        modules, sorted,
        "failures must be deterministically ordered"
    );
    assert!(failures.iter().any(|f| f.module.ends_with("alpha")));
    assert!(failures.iter().any(|f| f.module.ends_with("beta")));
    // Each failure carries its own structured errors and rendered source.
    for failure in &failures {
        assert!(
            !failure.errors.is_empty(),
            "each module carries structured errors"
        );
        assert!(failure.path.ends_with(format!(
            "{}.ab",
            failure.module.rsplit("::").next().unwrap()
        )));
    }
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

    let BuildError::TypeCheck { failures } = err else {
        panic!("expected a structured TypeCheck error, got: {err:?}");
    };
    // A single-module package fails in exactly one module.
    assert_eq!(failures.len(), 1, "expected one failing module");
    let failure = &failures[0];
    let source = &failure.source;
    let path = &failure.path;
    let errors = &failure.errors;

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
