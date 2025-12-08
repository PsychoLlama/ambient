//! Tab completion and hint support for the REPL.
//!
//! Provides:
//! - Ghost text hints showing the best completion
//! - Tab cycling through completion candidates
//! - Completion for keywords, types, abilities, modules, and user-defined symbols

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

use ambient_engine::compiler::ReplContext;
use ambient_engine::core_library::CoreLibrary;
use ambient_engine::manifest::Manifest;
use ambient_parser::TokenKind;

use super::highlighter::AmbientHighlighter;

/// A completion candidate with metadata for sorting.
#[derive(Debug, Clone)]
struct Candidate {
    /// The completion text to insert.
    text: String,
    /// Display text (may differ from insert text).
    display: String,
    /// Priority for sorting (lower = higher priority).
    priority: u8,
}

impl Candidate {
    fn new(text: impl Into<String>, priority: u8) -> Self {
        let text = text.into();
        Self {
            display: text.clone(),
            text,
            priority,
        }
    }

    fn with_display(text: impl Into<String>, display: impl Into<String>, priority: u8) -> Self {
        Self {
            text: text.into(),
            display: display.into(),
            priority,
        }
    }
}

/// State for tab-cycling through completions.
#[derive(Debug, Default)]
struct CycleState {
    /// The candidates for the current completion.
    candidates: Vec<Candidate>,
    /// Current index in the cycle.
    index: usize,
    /// The line content when cycling started.
    original_line: String,
    /// The cursor position when cycling started.
    original_pos: usize,
    /// The prefix being completed.
    prefix: String,
}

/// REPL completer with tab-cycling and ghost text hints.
pub struct ReplCompleter {
    /// Project root directory (where ambient.toml is, or provided dir).
    #[allow(dead_code)]
    project_dir: PathBuf,
    /// Source directory within the project.
    #[allow(dead_code)]
    src_dir: Option<PathBuf>,
    /// Discovered module paths (e.g., "utils", "utils.helper").
    module_paths: Vec<String>,
    /// Shared REPL context for user-defined symbols.
    repl_ctx: Arc<Mutex<ReplContext>>,
    /// Syntax highlighter.
    highlighter: AmbientHighlighter,
    /// State for tab-cycling.
    cycle_state: RefCell<CycleState>,
}

impl ReplCompleter {
    /// Create a new completer for the given project directory.
    pub fn new(project_dir: PathBuf, repl_ctx: Arc<Mutex<ReplContext>>) -> Self {
        let (src_dir, module_paths) = discover_modules(&project_dir);

        Self {
            project_dir,
            src_dir,
            module_paths,
            repl_ctx,
            highlighter: AmbientHighlighter,
            cycle_state: RefCell::new(CycleState::default()),
        }
    }

    /// Get all completion candidates for the current input.
    fn get_candidates(&self, line: &str, pos: usize) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        // Find the word being typed (including dots for qualified names).
        let before_cursor = &line[..pos];
        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |i| i + 1);
        let full_prefix = &before_cursor[word_start..];

        // Check if the prefix contains a dot (qualified name like "Console." or "pkg.utils")
        if let Some(dot_pos) = full_prefix.rfind('.') {
            let before_dot = &full_prefix[..dot_pos];
            let after_dot = &full_prefix[dot_pos + 1..];

            // After `pkg.` - complete module paths
            if before_dot == "pkg" || before_dot.starts_with("pkg.") {
                let module_prefix = if before_dot == "pkg" {
                    after_dot
                } else {
                    // pkg.foo.bar -> module prefix is "foo.bar" + after_dot
                    let rest = before_dot.strip_prefix("pkg.").unwrap_or("");
                    // We need to handle nested paths
                    if after_dot.is_empty() {
                        rest
                    } else {
                        // This is a bit tricky - for now just use after_dot
                        after_dot
                    }
                };
                candidates.extend(self.get_module_completions(module_prefix, "pkg."));
                return self.sort_candidates(candidates, after_dot);
            }

            // After `core.` - complete core library modules
            if before_dot == "core" {
                candidates.extend(get_core_module_completions(after_dot));
                return self.sort_candidates(candidates, after_dot);
            }

            // After ability name + dot (e.g., "Console.") - complete methods
            if TokenKind::builtin_abilities().contains(&before_dot) {
                candidates.extend(get_ability_method_completions(before_dot, after_dot));
                return self.sort_candidates(candidates, after_dot);
            }

            // Unknown qualified name - no completions
            return candidates;
        }

        // No dot - general completions based on the prefix
        let prefix = full_prefix;

        // General completions
        candidates.extend(get_keyword_completions(prefix));
        candidates.extend(get_type_completions(prefix));
        candidates.extend(get_ability_completions(prefix));
        candidates.extend(get_import_prefix_completions(prefix));
        candidates.extend(self.get_repl_symbol_completions(prefix));

        self.sort_candidates(candidates, prefix)
    }

    /// Sort candidates by relevance.
    fn sort_candidates(&self, mut candidates: Vec<Candidate>, prefix: &str) -> Vec<Candidate> {
        // Sort by: exact prefix match first, then by priority, then alphabetically
        candidates.sort_by(|a, b| {
            let a_exact = a.text.starts_with(prefix);
            let b_exact = b.text.starts_with(prefix);

            match (a_exact, b_exact) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a
                    .priority
                    .cmp(&b.priority)
                    .then_with(|| a.text.cmp(&b.text)),
            }
        });

        candidates
    }

    /// Get module path completions for `pkg.` prefix.
    fn get_module_completions(&self, prefix: &str, insert_prefix: &str) -> Vec<Candidate> {
        self.module_paths
            .iter()
            .filter(|p| p.starts_with(prefix))
            .map(|p| {
                Candidate::with_display(
                    format!("{insert_prefix}{p}"),
                    p.clone(),
                    20, // Module priority
                )
            })
            .collect()
    }

    /// Get completions for REPL-defined symbols.
    fn get_repl_symbol_completions(&self, prefix: &str) -> Vec<Candidate> {
        let ctx = self.repl_ctx.lock().unwrap();
        ctx.function_hashes
            .keys()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Candidate::new(name.to_string(), 5)) // User symbols high priority
            .collect()
    }
}

/// Discover modules in a project directory.
fn discover_modules(dir: &Path) -> (Option<PathBuf>, Vec<String>) {
    // Try to find ambient.toml by walking up
    let mut current = dir;
    let manifest_path = loop {
        let candidate = current.join("ambient.toml");
        if candidate.exists() {
            break Some(candidate);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break None,
        }
    };

    let Some(manifest_path) = manifest_path else {
        return (None, Vec::new());
    };

    let Ok(manifest) = Manifest::from_file(&manifest_path) else {
        return (None, Vec::new());
    };

    let project_root = manifest_path.parent().unwrap();
    let src_dir = project_root.join(&manifest.src_dir);

    if !src_dir.is_dir() {
        return (Some(src_dir), Vec::new());
    }

    let modules = discover_ab_files(&src_dir, &src_dir);
    (Some(src_dir), modules)
}

/// Recursively discover .ab files and convert to module paths.
fn discover_ab_files(dir: &Path, src_root: &Path) -> Vec<String> {
    let mut modules = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return modules;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            modules.extend(discover_ab_files(&path, src_root));
        } else if path.extension().is_some_and(|ext| ext == "ab") {
            if let Some(module_path) = path_to_module(&path, src_root) {
                // Skip "main" as it's the root module
                if module_path != "main" {
                    modules.push(module_path);
                }
            }
        }
    }

    modules
}

/// Convert a file path to a module path string.
fn path_to_module(path: &Path, src_root: &Path) -> Option<String> {
    let relative = path.strip_prefix(src_root).ok()?;
    let mut segments = Vec::new();

    for component in relative.components() {
        if let std::path::Component::Normal(s) = component {
            let name = s.to_str()?;
            let name = name.strip_suffix(".ab").unwrap_or(name);
            segments.push(name);
        }
    }

    Some(segments.join("."))
}

/// Get keyword completions.
/// Excludes import-only keywords (pkg, core, self, super) since they can't
/// be used as standalone expressions in the REPL.
fn get_keyword_completions(prefix: &str) -> Vec<Candidate> {
    const IMPORT_ONLY_KEYWORDS: &[&str] = &["pkg", "core", "self", "super"];
    TokenKind::all_keywords()
        .iter()
        .filter(|kw| kw.starts_with(prefix) && !IMPORT_ONLY_KEYWORDS.contains(kw))
        .map(|kw| Candidate::new(*kw, 30)) // Keywords lower priority
        .collect()
}

/// Get built-in type completions.
fn get_type_completions(prefix: &str) -> Vec<Candidate> {
    TokenKind::builtin_types()
        .iter()
        .filter(|ty| ty.starts_with(prefix))
        .map(|ty| Candidate::new(*ty, 25)) // Types medium priority
        .collect()
}

/// Get ability completions.
fn get_ability_completions(prefix: &str) -> Vec<Candidate> {
    TokenKind::builtin_abilities()
        .iter()
        .filter(|ab| ab.starts_with(prefix))
        .map(|ab| Candidate::new(*ab, 10)) // Abilities high priority
        .collect()
}

/// Get core library module completions.
fn get_core_module_completions(prefix: &str) -> Vec<Candidate> {
    CoreLibrary::available_modules()
        .into_iter()
        .filter(|m| m.starts_with(prefix))
        .map(|m| Candidate::new(m, 15))
        .collect()
}

/// Get import prefix completions (pkg, core).
/// Note: `self` and `super` are not included here since they are keywords
/// that only make sense in `use` statements, not as standalone expressions.
fn get_import_prefix_completions(prefix: &str) -> Vec<Candidate> {
    ["pkg", "core"]
        .iter()
        .filter(|p| p.starts_with(prefix))
        .map(|p| Candidate::new(*p, 35)) // Import prefixes low priority
        .collect()
}

/// Get ability method completions.
fn get_ability_method_completions(ability: &str, prefix: &str) -> Vec<Candidate> {
    let methods: &[(&str, &str)] = match ability {
        "Console" => &[
            ("print!", "print a message to stdout"),
            ("eprint!", "print a message to stderr"),
            ("println!", "print a message with newline"),
        ],
        "Exception" => &[("throw!", "throw an exception")],
        "Time" => &[
            ("now!", "get current timestamp"),
            ("wait!", "wait for a duration"),
        ],
        "Random" => &[
            ("seed!", "get a random number 0.0 to 1.0"),
            ("in_range!", "get random number in range"),
        ],
        "Async" => &[
            ("all!", "wait for all operations"),
            ("race!", "race operations, first wins"),
        ],
        "Log" => &[
            ("debug!", "log debug message"),
            ("info!", "log info message"),
            ("warn!", "log warning message"),
            ("error!", "log error message"),
        ],
        "Filesystem" => &[
            ("read!", "read file contents"),
            ("write!", "write file contents"),
            ("exists!", "check if file exists"),
        ],
        "Network" => &[("fetch!", "fetch a URL")],
        _ => &[],
    };

    methods
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, _)| Candidate::new(*name, 5))
        .collect()
}

impl Helper for ReplCompleter {}

impl Completer for ReplCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let mut state = self.cycle_state.borrow_mut();

        // Find the word being typed for replacement position.
        let before_cursor = &line[..pos];
        let word_start = before_cursor
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |i| i + 1);
        let prefix = &before_cursor[word_start..];

        // Check if we're continuing a cycle or starting fresh.
        let is_continuing = state.original_line == line
            && state.original_pos == pos
            && !state.candidates.is_empty();

        if is_continuing {
            // Cycle to next candidate.
            state.index = (state.index + 1) % state.candidates.len();
        } else {
            // Start fresh completion.
            let candidates = self.get_candidates(line, pos);
            state.candidates = candidates;
            state.index = 0;
            state.original_line = line.to_string();
            state.original_pos = pos;
            state.prefix = prefix.to_string();
        }

        if state.candidates.is_empty() {
            return Ok((pos, Vec::new()));
        }

        // Return the current candidate.
        let candidate = &state.candidates[state.index];
        let pair = Pair {
            display: format!(
                "{} ({}/{})",
                candidate.display,
                state.index + 1,
                state.candidates.len()
            ),
            replacement: candidate.text.clone(),
        };

        Ok((word_start, vec![pair]))
    }
}

impl Hinter for ReplCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        // Only show hint if cursor is at the end of the line.
        if pos != line.len() {
            return None;
        }

        // Find the word being typed.
        let word_start = line
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map_or(0, |i| i + 1);
        let prefix = &line[word_start..];

        // Need at least one character to show a hint.
        if prefix.is_empty() {
            return None;
        }

        let candidates = self.get_candidates(line, pos);

        // Show the best match as ghost text.
        candidates.first().map(|c| {
            // Only show the suffix that would be added.
            let suffix = if c.text.starts_with(prefix) {
                &c.text[prefix.len()..]
            } else {
                &c.text
            };
            // Return with dim gray color.
            format!("\x1b[90m{suffix}\x1b[0m")
        })
    }
}

impl Validator for ReplCompleter {}

impl Highlighter for ReplCompleter {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        self.highlighter.highlight(line, pos)
    }

    fn highlight_char(&self, line: &str, pos: usize, forced: bool) -> bool {
        self.highlighter.highlight_char(line, pos, forced)
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        self.highlighter.highlight_prompt(prompt, default)
    }

    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Hint is already colored in the hint() method.
        Cow::Borrowed(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test completer with no project.
    fn test_completer() -> ReplCompleter {
        ReplCompleter::new(
            PathBuf::from("/nonexistent"),
            Arc::new(Mutex::new(ReplContext::new())),
        )
    }

    /// Helper to get candidate texts from a completer.
    fn get_candidate_texts(completer: &ReplCompleter, line: &str, pos: usize) -> Vec<String> {
        completer
            .get_candidates(line, pos)
            .into_iter()
            .map(|c| c.text)
            .collect()
    }

    // === Basic completion tests ===

    #[test]
    fn test_keyword_completions() {
        let completions = get_keyword_completions("le");
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].text, "let");
    }

    #[test]
    fn test_keyword_completions_empty_prefix() {
        let completions = get_keyword_completions("");
        // Should return all keywords except pkg, core, self, super
        assert!(completions.len() > 10);
        let texts: Vec<_> = completions.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"fn"));
        assert!(texts.contains(&"let"));
        assert!(texts.contains(&"if"));
        // Import-only keywords should be excluded
        assert!(!texts.contains(&"self"));
        assert!(!texts.contains(&"super"));
        assert!(!texts.contains(&"pkg"));
        assert!(!texts.contains(&"core"));
    }

    #[test]
    fn test_ability_completions() {
        let completions = get_ability_completions("Con");
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].text, "Console");
    }

    #[test]
    fn test_ability_completions_empty_prefix() {
        let completions = get_ability_completions("");
        let texts: Vec<_> = completions.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"Console"));
        assert!(texts.contains(&"Filesystem"));
        assert!(texts.contains(&"Time"));
        assert!(texts.contains(&"Random"));
    }

    #[test]
    fn test_ability_method_completions() {
        let completions = get_ability_method_completions("Console", "pr");
        assert_eq!(completions.len(), 2); // print!, println!
    }

    #[test]
    fn test_ability_method_completions_empty_prefix() {
        let completions = get_ability_method_completions("Console", "");
        assert_eq!(completions.len(), 3); // print!, eprint!, println!
    }

    #[test]
    fn test_core_module_completions() {
        let completions = get_core_module_completions("ma");
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].text, "math");
    }

    #[test]
    fn test_core_module_completions_empty_prefix() {
        let completions = get_core_module_completions("");
        let texts: Vec<_> = completions.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"math"));
        assert!(texts.contains(&"list"));
        assert!(texts.contains(&"option"));
        assert!(texts.contains(&"result"));
        assert!(texts.contains(&"string"));
    }

    #[test]
    fn test_import_prefix_completions() {
        let completions = get_import_prefix_completions("");
        let texts: Vec<_> = completions.iter().map(|c| c.text.as_str()).collect();
        assert!(texts.contains(&"pkg"));
        assert!(texts.contains(&"core"));
        // self and super should NOT be in import prefix completions
        assert!(!texts.contains(&"self"));
        assert!(!texts.contains(&"super"));
    }

    // === ReplCompleter integration tests ===

    #[test]
    fn test_completer_empty_input() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "", 0);
        // Should have many completions
        assert!(
            candidates.len() > 10,
            "Expected many completions, got {}",
            candidates.len()
        );
        // Should include keywords, types, and abilities
        assert!(candidates.contains(&"fn".to_string()));
        assert!(candidates.contains(&"let".to_string()));
        assert!(candidates.contains(&"Console".to_string()));
        assert!(candidates.contains(&"number".to_string()));
    }

    #[test]
    fn test_completer_partial_keyword() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "le", 2);
        assert!(candidates.contains(&"let".to_string()));
    }

    #[test]
    fn test_completer_partial_ability() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "Con", 3);
        assert!(candidates.contains(&"Console".to_string()));
        // Note: "const" does NOT match "Con" (case-sensitive)
        assert!(!candidates.contains(&"const".to_string()));
    }

    #[test]
    fn test_completer_partial_keyword_const() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "con", 3);
        assert!(candidates.contains(&"const".to_string()));
        // Console doesn't match "con" (case-sensitive)
        assert!(!candidates.contains(&"Console".to_string()));
    }

    #[test]
    fn test_completer_after_console_dot() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "Console.", 8);
        // Should show Console methods
        assert!(candidates.iter().any(|c| c.contains("print!")));
        assert!(candidates.iter().any(|c| c.contains("println!")));
        // Should NOT show general completions
        assert!(!candidates.contains(&"fn".to_string()));
        assert!(!candidates.contains(&"let".to_string()));
    }

    #[test]
    fn test_completer_after_console_dot_with_prefix() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "Console.pr", 10);
        assert!(candidates.iter().any(|c| c.contains("print!")));
        assert!(candidates.iter().any(|c| c.contains("println!")));
        // eprint! should not match "pr" prefix
        assert!(!candidates.iter().any(|c| c.contains("eprint!")));
    }

    #[test]
    fn test_completer_after_core_dot() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "core.", 5);
        // Should show core modules
        assert!(candidates.contains(&"math".to_string()));
        assert!(candidates.contains(&"list".to_string()));
        // Should NOT show general completions
        assert!(!candidates.contains(&"fn".to_string()));
    }

    #[test]
    fn test_completer_after_core_dot_with_prefix() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "core.ma", 7);
        assert!(candidates.contains(&"math".to_string()));
        assert!(!candidates.contains(&"list".to_string()));
    }

    #[test]
    fn test_completer_after_pkg_dot() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "pkg.", 4);
        // With no project, should return empty for pkg completions
        // (no modules discovered)
        // But the key is it shouldn't show general completions
        assert!(!candidates.contains(&"fn".to_string()));
    }

    #[test]
    fn test_completer_typing_pkg() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "pk", 2);
        assert!(candidates.contains(&"pkg".to_string()));
    }

    #[test]
    fn test_completer_typing_core() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "cor", 3);
        assert!(candidates.contains(&"core".to_string()));
    }

    #[test]
    fn test_completer_does_not_include_self_super() {
        let completer = test_completer();
        let candidates = get_candidate_texts(&completer, "", 0);
        // self and super should never appear in completions
        assert!(!candidates.contains(&"self".to_string()));
        assert!(!candidates.contains(&"super".to_string()));

        // Even when typing "se" or "su"
        let candidates = get_candidate_texts(&completer, "se", 2);
        assert!(!candidates.contains(&"self".to_string()));

        let candidates = get_candidate_texts(&completer, "su", 2);
        assert!(!candidates.contains(&"super".to_string()));
    }

    #[test]
    fn test_completer_in_expression_context() {
        let completer = test_completer();
        // After typing "let x = ", should get expression completions
        let candidates = get_candidate_texts(&completer, "let x = ", 8);
        assert!(candidates.contains(&"Console".to_string()));
        assert!(candidates.contains(&"true".to_string()));
        assert!(candidates.contains(&"false".to_string()));
    }

    #[test]
    fn test_completer_repl_symbols() {
        let repl_ctx = Arc::new(Mutex::new(ReplContext::new()));
        {
            let mut ctx = repl_ctx.lock().unwrap();
            ctx.register_function(Arc::from("my_function"), blake3::hash(b"test"));
            ctx.register_constant(Arc::from("MY_CONST"), blake3::hash(b"test2"));
        }
        let completer = ReplCompleter::new(PathBuf::from("/nonexistent"), repl_ctx);

        let candidates = get_candidate_texts(&completer, "", 0);
        assert!(candidates.contains(&"my_function".to_string()));
        assert!(candidates.contains(&"MY_CONST".to_string()));

        // Partial match
        let candidates = get_candidate_texts(&completer, "my_", 3);
        assert!(candidates.contains(&"my_function".to_string()));
        assert!(!candidates.contains(&"MY_CONST".to_string()));
    }

    #[test]
    fn test_completer_with_real_project() {
        // Test with the multi_module example project
        let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/multi_module");

        if !project_dir.exists() {
            // Skip if example doesn't exist
            return;
        }

        let completer = ReplCompleter::new(project_dir, Arc::new(Mutex::new(ReplContext::new())));

        // Check that modules were discovered
        assert!(
            !completer.module_paths.is_empty(),
            "Should discover modules in multi_module example"
        );

        // Should find list_utils, math_utils, statistics
        assert!(
            completer
                .module_paths
                .iter()
                .any(|p| p.contains("list_utils")),
            "Should find list_utils module, found: {:?}",
            completer.module_paths
        );

        // Test pkg. completion - should return module names
        let candidates = get_candidate_texts(&completer, "pkg.", 4);
        assert!(
            candidates.iter().any(|c| c.contains("list_utils")),
            "pkg. should complete to list_utils, got: {:?}",
            candidates
        );

        // Test pkg.list completion - filter by prefix
        let candidates = get_candidate_texts(&completer, "pkg.list", 8);
        assert!(
            candidates.iter().any(|c| c.contains("list_utils")),
            "pkg.list should complete to list_utils, got: {:?}",
            candidates
        );
    }

    #[test]
    fn test_module_completions_return_full_path() {
        // Test that module completions include the pkg. prefix for insertion
        let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/multi_module");

        if !project_dir.exists() {
            return;
        }

        let completer = ReplCompleter::new(project_dir, Arc::new(Mutex::new(ReplContext::new())));

        eprintln!("Discovered modules: {:?}", completer.module_paths);

        // When completing "pkg.", the returned candidates should work for insertion
        let candidates = completer.get_candidates("pkg.", 4);
        eprintln!(
            "Candidates for 'pkg.': {:?}",
            candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
        );

        // Find the list_utils candidate
        let list_utils = candidates.iter().find(|c| c.text.contains("list_utils"));
        assert!(
            list_utils.is_some(),
            "Should have list_utils candidate, got: {:?}",
            candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
        );

        // Test "pkg.list" partial completion
        let candidates = completer.get_candidates("pkg.list", 8);
        eprintln!(
            "Candidates for 'pkg.list': {:?}",
            candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
        );
        assert!(
            candidates.iter().any(|c| c.text.contains("list_utils")),
            "pkg.list should complete to pkg.list_utils"
        );

        // The text should be the full replacement value
        let candidate = candidates
            .iter()
            .find(|c| c.text.contains("list_utils"))
            .unwrap();
        assert_eq!(
            candidate.text, "pkg.list_utils",
            "Text should be full path for replacement"
        );
    }

    #[test]
    fn test_completer_sorting_relevance() {
        let completer = test_completer();
        // When typing "R", Random (ability, priority 10) should come before Result (type, priority 25)
        let candidates = completer.get_candidates("R", 1);
        let texts: Vec<_> = candidates.iter().map(|c| c.text.as_str()).collect();

        // Find positions
        let random_pos = texts.iter().position(|t| *t == "Random");
        let result_pos = texts.iter().position(|t| *t == "Result");

        assert!(random_pos.is_some(), "Random should be in completions");
        assert!(result_pos.is_some(), "Result should be in completions");
        // Random (ability, priority 10) should come before Result (type, priority 25)
        assert!(
            random_pos.unwrap() < result_pos.unwrap(),
            "Random should come before Result (abilities have higher priority than types)"
        );
    }
}
