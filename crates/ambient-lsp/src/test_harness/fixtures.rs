//! Fixture parsing for LSP tests.
//!
//! Supports cursor markers in source code:
//! - `/*|*/` - Anonymous cursor (numbered 0, 1, 2...)
//! - `/*name*/` - Named cursor

use std::collections::HashMap;

/// A cursor position extracted from source markers.
#[derive(Debug, Clone)]
pub struct Cursor {
    /// The marker name (e.g., "hover", "def", "0", "1").
    pub name: String,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character/column (0-indexed).
    pub character: u32,
}

/// Parse markers from source code.
///
/// Returns the cleaned source (with markers removed) and a list of cursor positions.
///
/// # Marker Syntax
///
/// - `/*|*/` - Anonymous cursor (numbered automatically: 0, 1, 2...)
/// - `/*name*/` - Named cursor
///
/// # Example
///
/// ```ignore
/// let source = "fn foo() { let x/*def*/ = 1; x/*use*/ }";
/// let (clean, cursors) = parse_markers(source);
/// assert_eq!(clean, "fn foo() { let x = 1; x }");
/// assert_eq!(cursors.len(), 2);
/// ```
pub fn parse_markers(source: &str) -> (String, Vec<Cursor>) {
    let mut clean = String::new();
    let mut cursors = Vec::new();
    let mut anonymous_count = 0;

    let mut chars = source.chars().peekable();
    let mut line: u32 = 0;
    let mut character: u32 = 0;

    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'*') {
            // Potential marker start
            chars.next(); // consume '*'

            // Read until '*/'
            let mut marker_content = String::new();
            let mut found_end = false;

            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next(); // consume '/'
                    found_end = true;
                    break;
                }
                marker_content.push(c);
            }

            if found_end && is_cursor_marker(&marker_content) {
                // This is a cursor marker
                let name = if marker_content == "|" {
                    let name = anonymous_count.to_string();
                    anonymous_count += 1;
                    name
                } else {
                    marker_content
                };

                cursors.push(Cursor {
                    name,
                    line,
                    character,
                });
            } else {
                // Not a cursor marker, keep it in the output
                clean.push('/');
                clean.push('*');
                clean.push_str(&marker_content);
                if found_end {
                    clean.push('*');
                    clean.push('/');
                }
                // Update character count for the content we just added
                for c in marker_content.chars() {
                    if c == '\n' {
                        line += 1;
                        character = 0;
                    } else {
                        character += 1;
                    }
                }
                character += 4; // for /* and */
            }
        } else {
            clean.push(ch);
            if ch == '\n' {
                line += 1;
                character = 0;
            } else {
                character += 1;
            }
        }
    }

    (clean, cursors)
}

/// Check if a comment content is a cursor marker.
///
/// Cursor markers are:
/// - `|` (anonymous)
/// - Alphanumeric identifiers (named)
fn is_cursor_marker(content: &str) -> bool {
    if content == "|" {
        return true;
    }

    // Named markers must be non-empty and contain only alphanumeric/underscore chars
    !content.is_empty() && content.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Get a cursor by name from a list of cursors.
pub fn get_cursor_by_name<'a>(cursors: &'a [Cursor], name: &str) -> Option<&'a Cursor> {
    cursors.iter().find(|c| c.name == name)
}

/// Parse markers from multiple files.
///
/// Returns a map of path -> (clean_source, cursors).
#[allow(dead_code)]
pub fn parse_markers_multi(files: &[(String, String)]) -> HashMap<String, (String, Vec<Cursor>)> {
    files
        .iter()
        .map(|(path, content)| {
            let (clean, cursors) = parse_markers(content);
            (path.clone(), (clean, cursors))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_anonymous_marker() {
        let (clean, cursors) = parse_markers("let x/*|*/ = 1");
        assert_eq!(clean, "let x = 1");
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].name, "0");
        assert_eq!(cursors[0].line, 0);
        assert_eq!(cursors[0].character, 5);
    }

    #[test]
    fn test_parse_named_marker() {
        let (clean, cursors) = parse_markers("let x/*def*/ = 1");
        assert_eq!(clean, "let x = 1");
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].name, "def");
        assert_eq!(cursors[0].line, 0);
        assert_eq!(cursors[0].character, 5);
    }

    #[test]
    fn test_parse_multiple_markers() {
        let (clean, cursors) = parse_markers("let x/*def*/ = 1; x/*use*/");
        assert_eq!(clean, "let x = 1; x");
        assert_eq!(cursors.len(), 2);
        assert_eq!(cursors[0].name, "def");
        assert_eq!(cursors[1].name, "use");
    }

    #[test]
    fn test_parse_multiline() {
        let source = "fn foo() {\n    let x/*def*/ = 1;\n    x/*use*/\n}";
        let (clean, cursors) = parse_markers(source);
        assert_eq!(clean, "fn foo() {\n    let x = 1;\n    x\n}");
        assert_eq!(cursors.len(), 2);

        // def is on line 1, after "    let x"
        assert_eq!(cursors[0].name, "def");
        assert_eq!(cursors[0].line, 1);
        assert_eq!(cursors[0].character, 9);

        // use is on line 2, after "    x"
        assert_eq!(cursors[1].name, "use");
        assert_eq!(cursors[1].line, 2);
        assert_eq!(cursors[1].character, 5);
    }

    #[test]
    fn test_preserve_non_marker_comments() {
        let (clean, cursors) = parse_markers("let x = 1 /* this is a comment */");
        // Non-marker comments should be preserved
        assert!(clean.contains("/* this is a comment */"));
        assert_eq!(cursors.len(), 0);
    }

    #[test]
    fn test_mixed_anonymous_and_named() {
        let (clean, cursors) = parse_markers("a/*|*/ b/*foo*/ c/*|*/");
        assert_eq!(clean, "a b c");
        assert_eq!(cursors.len(), 3);
        assert_eq!(cursors[0].name, "0");
        assert_eq!(cursors[1].name, "foo");
        assert_eq!(cursors[2].name, "1");
    }

    #[test]
    fn test_get_cursor_by_name() {
        let (_, cursors) = parse_markers("let x/*def*/ = 1; x/*use*/");
        assert!(get_cursor_by_name(&cursors, "def").is_some());
        assert!(get_cursor_by_name(&cursors, "use").is_some());
        assert!(get_cursor_by_name(&cursors, "nonexistent").is_none());
    }
}
