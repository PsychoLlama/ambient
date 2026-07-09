//! Value formatting utilities for display.
//!
//! Provides formatting for runtime values in different contexts:
//! - `format_value`: Plain text, strings shown without quotes (for Stdio.out!)
//! - `format_value_display`: Plain text, strings with quotes (for REPL result display)
//! - `format_value_colored`: Syntax highlighted with ANSI colors (for terminal output)

use std::fmt::Write;

use crate::value::{ModuleExportKind, Value};

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
    /// Plain text, strings without quotes (for Stdio.out!).
    Plain,
    /// Display mode, strings with quotes (for REPL display).
    Display,
    /// Colored display mode with ANSI escape codes.
    Colored,
}

/// Format a value as plain text (strings without quotes).
///
/// This is used for Stdio.out! where we want to display the string
/// content directly without quoting.
#[must_use]
pub fn format_value(value: &Value) -> String {
    format_value_impl(value, FormatMode::Plain)
}

/// Format a value for display (strings with quotes).
///
/// This produces output similar to Python's `repr()` - strings are quoted
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
        Value::Unit => format_unit(color),
        Value::Bool(b) => format_bool(*b, color),
        Value::Number(n) => format_number(*n, color),
        Value::String(s) => format_string(s, quote_strings, color),
        Value::Binary(b) => format_bytes(b, color),
        Value::Tuple(elements) => format_sequence(elements, "(", ")", mode),
        Value::List(elements) => format_sequence(elements, "[", "]", mode),
        Value::Record(fields) => format_record(fields, color, mode),
        Value::FunctionRef(hash) | Value::ObjectRef(hash) => format_function_ref(hash, color),
        Value::AbilityRef(id) => {
            let hex = id.short_hex();
            if color {
                format!("{}<ability {hex}>{}", colors::DIM, colors::RESET)
            } else {
                format!("<ability {hex}>")
            }
        }
        Value::SuspendedAbility(ability) => format_suspended_ability(ability, color),
        Value::Continuation(_) => format_continuation(color),
        Value::Closure(closure) => format_closure(closure, color),
        Value::Handler(handler) => format_handler(handler, color),
        Value::Map(map) => format_map(map, color, mode),
        Value::Set(set) => format_set(set, color, mode),
        Value::Enum(e) => format_enum(e, color, mode),
        Value::Module(m) => format_module(m, color),
        Value::ModuleMember(m) => format_module_member(m, color),
    }
}

fn format_unit(color: bool) -> String {
    if color {
        format!("{}(){}", colors::DIM, colors::RESET)
    } else {
        "()".to_string()
    }
}

fn format_bool(b: bool, color: bool) -> String {
    if color {
        format!("{}{b}{}", colors::MAGENTA, colors::RESET)
    } else {
        b.to_string()
    }
}

fn format_number(n: f64, color: bool) -> String {
    let formatted = if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        n.to_string()
    };
    if color {
        format!("{}{formatted}{}", colors::YELLOW, colors::RESET)
    } else {
        formatted
    }
}

fn format_string(s: &str, quote: bool, color: bool) -> String {
    if quote {
        let escaped = escape_string(s);
        if color {
            format!("{}\"{escaped}\"{}", colors::GREEN, colors::RESET)
        } else {
            format!("\"{escaped}\"")
        }
    } else {
        s.to_string()
    }
}

fn format_bytes(bytes: &[u8], color: bool) -> String {
    // Format as hex string for readability
    use std::fmt::Write;
    let hex = bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    });
    let len = bytes.len();
    if color {
        format!(
            "{}Binary{}<{}{len} bytes{}: {}{hex}{}>",
            colors::DIM,
            colors::RESET,
            colors::CYAN,
            colors::RESET,
            colors::GREEN,
            colors::RESET,
        )
    } else {
        format!("Binary<{len} bytes: {hex}>")
    }
}

fn format_sequence(elements: &[Value], open: &str, close: &str, mode: FormatMode) -> String {
    let parts: Vec<String> = elements
        .iter()
        .map(|v| format_value_impl(v, mode))
        .collect();
    let joined = parts.join(", ");
    format!("{open}{joined}{close}")
}

fn format_record(
    fields: &std::collections::HashMap<std::sync::Arc<str>, Value>,
    color: bool,
    mode: FormatMode,
) -> String {
    let mut parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            let v_str = format_value_impl(v, mode);
            if color {
                format!("{}{k}{}: {v_str}", colors::CYAN, colors::RESET)
            } else {
                format!("{k}: {v_str}")
            }
        })
        .collect();
    parts.sort();
    let joined = parts.join(", ");
    format!("{{ {joined} }}")
}

fn format_function_ref(hash: &blake3::Hash, color: bool) -> String {
    let hash_str = &hash.to_string()[..8];
    if color {
        format!("{}<fn {hash_str}>{}", colors::DIM, colors::RESET)
    } else {
        format!("<fn {hash_str}>")
    }
}

fn format_suspended_ability(ability: &crate::value::SuspendedAbility, color: bool) -> String {
    let ability_id = ability.ability_id.short_hex();
    let method_id = ability.method_id;
    let arg_count = ability.args.len();
    if color {
        format!(
            "{}<ability {ability_id}:{method_id} with {arg_count} args>{}",
            colors::DIM,
            colors::RESET
        )
    } else {
        format!("<ability {ability_id}:{method_id} with {arg_count} args>")
    }
}

fn format_continuation(color: bool) -> String {
    if color {
        format!("{}<continuation>{}", colors::DIM, colors::RESET)
    } else {
        "<continuation>".to_string()
    }
}

fn format_closure(closure: &crate::value::Closure, color: bool) -> String {
    let hash_str = &closure.function_hash.to_string()[..8];
    let capture_count = closure.environment.len();
    if color {
        format!(
            "{}<closure {hash_str} [{capture_count} captures]>{}",
            colors::DIM,
            colors::RESET
        )
    } else {
        format!("<closure {hash_str} [{capture_count} captures]>")
    }
}

fn format_handler(handler: &crate::value::HandlerValue, color: bool) -> String {
    let ability_id = handler.ability_id;
    let method_count = handler.methods.len();
    if color {
        format!(
            "{}<handler #{ability_id} [{method_count} methods]>{}",
            colors::DIM,
            colors::RESET
        )
    } else {
        format!("<handler #{ability_id} [{method_count} methods]>")
    }
}

fn format_map(map: &crate::value::MapValue, color: bool, mode: FormatMode) -> String {
    let mut parts: Vec<String> = map
        .entries
        .iter()
        .map(|(k, v)| {
            let k_str = format_value_impl(k, mode);
            let v_str = format_value_impl(v, mode);
            if color {
                format!("{}{k_str}{}: {v_str}", colors::CYAN, colors::RESET)
            } else {
                format!("{k_str}: {v_str}")
            }
        })
        .collect();
    parts.sort();
    let joined = parts.join(", ");
    if color {
        format!("{}Map{} {{ {joined} }}", colors::BLUE, colors::RESET)
    } else {
        format!("Map {{ {joined} }}")
    }
}

fn format_set(set: &crate::value::SetValue, color: bool, mode: FormatMode) -> String {
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

fn format_enum(e: &crate::value::EnumValue, color: bool, mode: FormatMode) -> String {
    let type_color = if color { colors::BLUE } else { "" };
    let reset = if color { colors::RESET } else { "" };
    if let Some(payload) = e.payload.as_deref() {
        let payload_str = format_value_impl(payload, mode);
        format!(
            "{type_color}{}::{}{reset}({payload_str})",
            e.type_name, e.variant_name
        )
    } else {
        format!("{type_color}{}::{}{reset}", e.type_name, e.variant_name)
    }
}

fn format_module(m: &crate::value::ModuleValue, color: bool) -> String {
    let blue = if color { colors::BLUE } else { "" };
    let cyan = if color { colors::CYAN } else { "" };
    let dim = if color { colors::DIM } else { "" };
    let reset = if color { colors::RESET } else { "" };

    let mut result = format!("{blue}module{reset} {}", m.path);

    if m.exports.is_empty() {
        result.push_str(" {}");
    } else {
        result.push_str(" {\n");

        // Collect and sort exports for consistent display
        let mut sorted_exports: Vec<_> = m.exports.iter().collect();
        sorted_exports.sort_by(|a, b| {
            // Sort by kind first (functions, then consts, types, abilities, modules)
            let kind_order = |k: &ModuleExportKind| match k {
                ModuleExportKind::Function => 0,
                ModuleExportKind::Const => 1,
                ModuleExportKind::Type | ModuleExportKind::Enum => 2,
                ModuleExportKind::Ability => 3,
                ModuleExportKind::Module => 4,
                ModuleExportKind::Variant => 5,
            };
            kind_order(&a.kind)
                .cmp(&kind_order(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
        });

        for export in sorted_exports {
            match export.kind {
                ModuleExportKind::Function => {
                    if let Some(sig) = &export.signature {
                        let _ = writeln!(
                            result,
                            "  {cyan}fn{reset} {}{dim}{sig}{reset};",
                            export.name
                        );
                    } else {
                        let _ = writeln!(result, "  {cyan}fn{reset} {}();", export.name);
                    }
                }
                ModuleExportKind::Const => {
                    if let Some(sig) = &export.signature {
                        let _ = writeln!(result, "  {}: {cyan}{sig}{reset};", export.name);
                    } else {
                        let _ = writeln!(result, "  {};", export.name);
                    }
                }
                ModuleExportKind::Type => {
                    let _ = writeln!(result, "  {cyan}type{reset} {};", export.name);
                }
                ModuleExportKind::Enum => {
                    let _ = writeln!(result, "  {cyan}enum{reset} {};", export.name);
                }
                ModuleExportKind::Ability => {
                    let _ = writeln!(result, "  {cyan}ability{reset} {};", export.name);
                }
                ModuleExportKind::Module => {
                    let _ = writeln!(result, "  {dim}mod{reset} {};", export.name);
                }
                ModuleExportKind::Variant => {
                    // Skip variants, they're part of enums
                }
            }
        }

        result.push('}');
    }

    result
}

fn format_module_member(m: &crate::value::ModuleMemberRef, color: bool) -> String {
    let blue = if color { colors::BLUE } else { "" };
    let reset = if color { colors::RESET } else { "" };

    let kind_str = match m.kind {
        ModuleExportKind::Function => "fn",
        ModuleExportKind::Const => "const",
        ModuleExportKind::Type => "type",
        ModuleExportKind::Enum => "enum",
        ModuleExportKind::Variant => "variant",
        ModuleExportKind::Ability => "ability",
        ModuleExportKind::Module => "module",
    };

    format!("{blue}{kind_str}{reset} {}", m.path)
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
                // Use write! to avoid format_push_string lint
                let _ = write!(result, "\\x{:02x}", c as u32);
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
    #[allow(clippy::approx_constant)] // 3.14 is literal formatting input, not π
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
