//! Pattern-position path-prefix parsing.
//!
//! Expression position admits path-prefix-keyword heads (`pkg::m::T::V`,
//! `core::…`, `super::…`, `self::…`); pattern position must admit exactly the
//! same set so a qualified variant spells identically in a `match` arm and as a
//! constructor. Fail-fast and recovering parsers must agree.

use super::Parser;
use crate::cst::CstPatternKind;

/// Segment names of a variant pattern parsed from `src` (fail-fast).
fn variant_pat_segments(src: &str) -> Vec<String> {
    let mut parser = Parser::new(src).expect("lex");
    let pat = parser.parse_pattern().expect("parse pattern");
    match pat.kind {
        CstPatternKind::Variant { name, .. } => {
            name.segments.iter().map(|s| s.name.to_string()).collect()
        }
        other => panic!("expected variant pattern, got {other:?}"),
    }
}

#[test]
fn pattern_accepts_module_qualified_variant() {
    assert_eq!(
        variant_pat_segments("shapes::Shape::Circle(r)"),
        ["shapes", "Shape", "Circle"]
    );
}

#[test]
fn pattern_accepts_pkg_rooted_variant() {
    // The gap this closes: `pkg`-rooted paths parsed in expressions but not
    // patterns. The head keyword must survive as the first path segment so
    // lowering/resolve treat it exactly like the expression spelling.
    assert_eq!(
        variant_pat_segments("pkg::shapes::Shape::Circle(r)"),
        ["pkg", "shapes", "Shape", "Circle"]
    );
}

#[test]
fn pattern_accepts_all_prefix_keyword_roots() {
    for prefix in ["pkg", "core", "super", "self"] {
        let src = format!("{prefix}::shapes::Shape::Dot");
        assert_eq!(
            variant_pat_segments(&src),
            [prefix, "shapes", "Shape", "Dot"],
            "prefix {prefix}"
        );
    }
}

/// Does `pat` parse cleanly in a `match` arm through both the fail-fast and
/// recovering parsers? They must never disagree.
fn pattern_parses(pat: &str) -> (bool, bool) {
    let src = format!("fn f(s: X): Number {{ match s {{ {pat} => 0, _ => 1 }} }}");
    let ff = crate::parse(&src).is_ok();
    let rec = crate::parse_recovering(&src).errors.is_empty();
    (ff, rec)
}

#[test]
fn pattern_prefix_parsers_agree() {
    for pat in [
        "shapes::Shape::Circle(r)",
        "pkg::shapes::Shape::Circle(r)",
        "core::shapes::Shape::Dot",
        "super::shapes::Shape::Dot",
        "self::shapes::Shape::Dot",
        "Circle(r)",
        "None",
    ] {
        let (ff, rec) = pattern_parses(pat);
        assert!(ff, "fail-fast should accept `{pat}`");
        assert!(rec, "recovering should accept `{pat}`");
    }
}

#[test]
fn pattern_malformed_prefix_errors_cleanly() {
    // A lone prefix keyword or a dangling `::` is not a pattern. Both parsers
    // must reject without panicking.
    for pat in ["pkg", "pkg::", "pkg::::x", "core::"] {
        let (ff, rec) = pattern_parses(pat);
        assert!(!ff, "fail-fast should reject `{pat}`");
        assert!(!rec, "recovering should reject `{pat}`");
    }
}
