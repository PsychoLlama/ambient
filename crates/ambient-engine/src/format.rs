//! Value formatting utilities for display.
//!
//! Provides formatting for runtime values in different contexts:
//! - `format_value`: Plain text, strings shown without quotes (for Console.print!)
//! - `format_value_display`: Plain text, strings with quotes (for REPL result display)
//! - `format_value_colored`: Syntax highlighted with ANSI colors (for terminal output)

use crate::value::Value;

// ANSI color codes
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const DIM: &str = "\x1b[2m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
}

/// Formatting mode for values.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatMode {
    /// Plain text, strings without quotes (for Console.print!).
    Plain,
    /// Display mode, strings with quotes (for REPL display).
    Display,
    /// Colored display mode with ANSI escape codes.
    Colored,
}

/// Format a value as plain text (strings without quotes).
///
/// This is used for Console.print! where we want to display the string
/// content directly without quoting.
#[must_use]
pub fn format_value(value: &Value) -> String {
    format_value_impl(value, FormatMode::Plain)
}

/// Format a value for display (strings with quotes).
///
/// This produces output similar to Python's repr() - strings are quoted
/// to distinguish them from other output.
#[must_use]
pub fn format_value_display(value: &Value) -> String {
    format_value_impl(value, FormatMode::Display)
}

/// Format a value with syntax highlighting (ANSI colors).
///
/// This produces colored output suitable for terminal display:
/// - Strings: green with quotes
/// - Numbers: yellow
/// - Booleans: magenta
/// - Unit: dim gray
/// - Type names: blue
/// - Record keys: cyan
#[must_use]
pub fn format_value_colored(value: &Value) -> String {
    format_value_impl(value, FormatMode::Colored)
}

fn format_value_impl(value: &Value, mode: FormatMode) -> String {
    let color = mode == FormatMode::Colored;
    let quote_strings = mode != FormatMode::Plain;

    match value {
        Value::Unit => {
            if color {
                format!("{}(){}", colors::DIM, colors::RESET)
            } else {
                "()".to_string()
            }
        }
        Value::Bool(b) => {
            if color {
                format!("{}{}{}", colors::MAGENTA, b, colors::RESET)
            } else {
                b.to_string()
            }
        }
        Value::Number(n) => {
            let formatted = if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{n:.0}")
            } else {
                n.to_string()
            };
            if color {
                format!("{}{}{}", colors::YELLOW, formatted, colors::RESET)
            } else {
                formatted
            }
        }
        Value::String(s) => {
            if quote_strings {
                // Escape special characters for display
                let escaped = escape_string(s);
                if color {
                    format!("{}\"{}\"{}",colors::GREEN, escaped, colors::RESET)
                } else {
                    format!("\"{}\"", escaped)
                }
            } else {
                // Plain mode: just the string content
                s.to_string()
            }
        }
        Value::Tuple(elements) => {
            let parts: Vec<String> = elements
                .iter()
                .map(|v| format_value_impl(v, mode))
                .collect();
            let joined = parts.join(", ");
            format!("({joined})")
        }
        Value::Record(fields) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| {
                    let v_str = format_value_impl(v, mode);
                    if color {
                        format!("{}{}{}: {}", colors::CYAN, k, colors::RESET, v_str)
                    } else {
                        format!("{k}: {v_str}")
                    }
                })
                .collect();
            parts.sort(); // Consistent ordering
            let joined = parts.join(", ");
            format!("{{ {joined} }}")
        }
        Value::FunctionRef(hash) => {
            let hash_str = &hash.to_string()[..8];
            if color {
                format!("{}<fn {}>{}", colors::DIM, hash_str, colors::RESET)
            } else {
                format!("<fn {hash_str}>")
            }
        }
        Value::SuspendedAbility(ability) => {
            let ability_id = ability.ability_id;
            let method_id = ability.method_id;
            let arg_count = ability.args.len();
            if color {
                format!(
                    "{}<ability {}:{} with {} args>{}",
                    colors::DIM,
                    ability_id,
                    method_id,
                    arg_count,
                    colors::RESET
                )
            } else {
                format!("<ability {ability_id}:{method_id} with {arg_count} args>")
            }
        }
        Value::Continuation(_) => {
            if color {
                format!("{}<continuation>{}", colors::DIM, colors::RESET)
            } else {
                "<continuation>".to_string()
            }
        }
        Value::Closure(closure) => {
            let hash_str = &closure.function_hash.to_string()[..8];
            let capture_count = closure.environment.len();
            if color {
                format!(
                    "{}<closure {} [{} captures]>{}",
                    colors::DIM,
                    hash_str,
                    capture_count,
                    colors::RESET
                )
            } else {
                format!("<closure {hash_str} [{capture_count} captures]>")
            }
        }
        Value::Handler(handler) => {
            let ability_id = handler.ability_id;
            let method_count = handler.methods.len();
            if color {
                format!(
                    "{}<handler #{} [{} methods]>{}",
                    colors::DIM,
                    ability_id,
                    method_count,
                    colors::RESET
                )
            } else {
                format!("<handler #{ability_id} [{method_count} methods]>")
            }
        }
        Value::List(elements) => {
            let parts: Vec<String> = elements
                .iter()
                .map(|v| format_value_impl(v, mode))
                .collect();
            let joined = parts.join(", ");
            format!("[{joined}]")
        }
        Value::Map(map) => {
            let mut parts: Vec<String> = map
                .entries
                .iter()
                .map(|(k, v)| {
                    let v_str = format_value_impl(v, mode);
                    if color {
                        format!("{}{}{}: {}", colors::CYAN, k, colors::RESET, v_str)
                    } else {
                        format!("{k}: {v_str}")
                    }
                })
                .collect();
            parts.sort(); // Consistent ordering
            let joined = parts.join(", ");
            if color {
                format!("{}Map{} {{ {joined} }}", colors::BLUE, colors::RESET)
            } else {
                format!("Map {{ {joined} }}")
            }
        }
        Value::Set(set) => {
            let parts: Vec<String> = set
                .elements
                .iter()
                .map(|v| format_value_impl(v, mode))
                .collect();
            let joined = parts.join(", ");
            if color {
                format!("{}Set{} {{ {joined} }}", colors::BLUE, colors::RESET)
            } else {
                format!("Set {{ {joined} }}")
            }
        }
        Value::Enum(e) => {
            let type_color = if color { colors::BLUE } else { "" };
            let reset = if color { colors::RESET } else { "" };
            if let Some(payload) = e.payload.as_deref() {
                format!(
                    "{}{}::{}{}({})",
                    type_color,
                    e.type_name,
                    e.variant_name,
                    reset,
                    format_value_impl(payload, mode)
                )
            } else {
                format!("{}{}::{}{}", type_color, e.type_name, e.variant_name, reset)
            }
        }
    }
}

/// Escape special characters in a string for display.
fn escape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            c if c.is_control() => {
                result.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_format_plain_primitives() {
        assert_eq!(format_value(&Value::Unit), "()");
        assert_eq!(format_value(&Value::Bool(true)), "true");
        assert_eq!(format_value(&Value::Bool(false)), "false");
        assert_eq!(format_value(&Value::Number(42.0)), "42");
        assert_eq!(format_value(&Value::Number(3.14)), "3.14");
        // Plain mode: strings without quotes
        assert_eq!(
            format_value(&Value::String(Arc::new("hello".to_string()))),
            "hello"
        );
    }

    #[test]
    fn test_format_display_primitives() {
        assert_eq!(format_value_display(&Value::Unit), "()");
        assert_eq!(format_value_display(&Value::Bool(true)), "true");
        // Display mode: strings with quotes
        assert_eq!(
            format_value_display(&Value::String(Arc::new("hello".to_string()))),
            "\"hello\""
        );
    }

    #[test]
    fn test_format_string_escapes() {
        // Escapes only apply in display/colored mode
        assert_eq!(
            format_value_display(&Value::String(Arc::new("line1\nline2".to_string()))),
            "\"line1\\nline2\""
        );
        assert_eq!(
            format_value_display(&Value::String(Arc::new("tab\there".to_string()))),
            "\"tab\\there\""
        );
        assert_eq!(
            format_value_display(&Value::String(Arc::new("quote\"here".to_string()))),
            "\"quote\\\"here\""
        );
    }

    #[test]
    fn test_format_collections() {
        assert_eq!(
            format_value(&Value::list(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ])),
            "[1, 2, 3]"
        );

        assert_eq!(
            format_value(&Value::tuple(vec![Value::Number(1.0), Value::Bool(true)])),
            "(1, true)"
        );
    }

    #[test]
    fn test_colored_output_has_escape_codes() {
        let colored = format_value_colored(&Value::Number(42.0));
        assert!(colored.contains("\x1b["));
        assert!(colored.contains("42"));

        let colored = format_value_colored(&Value::String(Arc::new("test".to_string())));
        assert!(colored.contains("\x1b[32m")); // green
        assert!(colored.contains("\"test\""));
    }
}
