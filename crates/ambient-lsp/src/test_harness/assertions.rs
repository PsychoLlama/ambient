//! Result types with fluent assertion methods for LSP tests.

use lsp_types::{CompletionItem, CompletionItemKind, Hover, Location};

use super::builder::LspTest;
use super::fixtures::Cursor;
use super::snapshot::{format_completions_snapshot, format_hover_snapshot};

/// Result type for hover queries.
pub struct HoverResult {
    pub(crate) test: LspTest,
    pub(crate) hover: Option<Hover>,
}

impl HoverResult {
    /// Assert that the hover content contains the expected type.
    ///
    /// # Panics
    ///
    /// Panics if hover is None or doesn't contain the expected type.
    #[must_use]
    pub fn expect_type(self, expected: &str) -> LspTest {
        let formatted = format_hover_snapshot(&self.hover);
        assert!(
            formatted.contains(expected),
            "Expected hover to contain type '{}', but got:\n{}",
            expected,
            formatted
        );
        self.test
    }

    /// Assert that the hover content contains the expected text.
    ///
    /// # Panics
    ///
    /// Panics if hover is None or doesn't contain the expected text.
    #[must_use]
    pub fn expect_contains(self, expected: &str) -> LspTest {
        let formatted = format_hover_snapshot(&self.hover);
        assert!(
            formatted.contains(expected),
            "Expected hover to contain '{}', but got:\n{}",
            expected,
            formatted
        );
        self.test
    }

    /// Assert that the hover content does NOT contain the given substring.
    ///
    /// # Panics
    ///
    /// Panics if the hover is present and contains the substring.
    #[must_use]
    pub fn expect_not_contains(self, unexpected: &str) -> LspTest {
        let formatted = format_hover_snapshot(&self.hover);
        assert!(
            !formatted.contains(unexpected),
            "Expected hover to NOT contain '{}', but got:\n{}",
            unexpected,
            formatted
        );
        self.test
    }

    /// Assert that no hover information is available.
    ///
    /// # Panics
    ///
    /// Panics if hover is Some.
    #[must_use]
    pub fn expect_none(self) -> LspTest {
        assert!(
            self.hover.is_none(),
            "Expected no hover, but got:\n{}",
            format_hover_snapshot(&self.hover)
        );
        self.test
    }

    /// Snapshot test the hover content.
    ///
    /// Uses insta for snapshot comparison.
    #[must_use]
    pub fn assert_snapshot(self) -> LspTest {
        let formatted = format_hover_snapshot(&self.hover);
        insta::assert_snapshot!(formatted);
        self.test
    }

    /// Get the raw hover response for custom assertions.
    #[must_use]
    pub fn raw(self) -> (LspTest, Option<Hover>) {
        (self.test, self.hover)
    }
}

/// Result type for go-to-definition queries.
pub struct DefinitionResult {
    pub(crate) test: LspTest,
    pub(crate) locations: Vec<Location>,
    pub(crate) all_cursors: Vec<(String, Cursor)>, // (file_path, cursor)
}

impl DefinitionResult {
    /// Assert that the definition jumps to the target cursor.
    ///
    /// # Panics
    ///
    /// Panics if no location matches the target cursor position.
    #[must_use]
    pub fn expect_target(self, target_name: &str) -> Self {
        let target = self
            .all_cursors
            .iter()
            .find(|(_, c)| c.name == target_name)
            .map(|(_, c)| c);

        let Some(target) = target else {
            panic!(
                "Target cursor '{}' not found. Available cursors: {:?}",
                target_name,
                self.all_cursors
                    .iter()
                    .map(|(_, c)| &c.name)
                    .collect::<Vec<_>>()
            );
        };

        let found = self.locations.iter().any(|loc| {
            loc.range.start.line == target.line && loc.range.start.character == target.character
        });

        assert!(
            found,
            "Expected definition to jump to cursor '{}' at {}:{}, but got locations: {:?}",
            target_name, target.line, target.character, self.locations
        );

        self
    }

    /// Assert that the definition is in a specific file.
    ///
    /// # Panics
    ///
    /// Panics if no location matches the expected file.
    #[must_use]
    pub fn expect_file(self, filename: &str) -> Self {
        let found = self
            .locations
            .iter()
            .any(|loc| loc.uri.as_str().ends_with(filename));

        assert!(
            found,
            "Expected definition in file '{}', but got: {:?}",
            filename,
            self.locations
                .iter()
                .map(|l| l.uri.as_str())
                .collect::<Vec<_>>()
        );

        self
    }

    /// Assert that no definition was found.
    ///
    /// # Panics
    ///
    /// Panics if any location was returned.
    #[must_use]
    pub fn expect_none(self) -> LspTest {
        assert!(
            self.locations.is_empty(),
            "Expected no definition, but got: {:?}",
            self.locations
        );
        self.test
    }

    /// Finish and return to the test builder.
    #[must_use]
    pub fn done(self) -> LspTest {
        self.test
    }

    /// Get the raw locations for custom assertions.
    #[must_use]
    pub fn raw(self) -> (LspTest, Vec<Location>) {
        (self.test, self.locations)
    }
}

/// Result type for completion queries.
pub struct CompletionResult {
    pub(crate) test: LspTest,
    pub(crate) items: Vec<CompletionItem>,
}

impl CompletionResult {
    /// Assert that a completion item with the given label exists.
    ///
    /// # Panics
    ///
    /// Panics if no item with the label is found.
    #[must_use]
    pub fn expect_item(self, label: &str) -> Self {
        let found = self.items.iter().any(|item| item.label == label);
        assert!(
            found,
            "Expected completion item '{}', but got: {:?}",
            label,
            self.items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        self
    }

    /// Assert that multiple completion items exist.
    ///
    /// # Panics
    ///
    /// Panics if any of the labels is not found.
    #[must_use]
    pub fn expect_items(self, labels: &[&str]) -> Self {
        for label in labels {
            let found = self.items.iter().any(|item| item.label == *label);
            assert!(
                found,
                "Expected completion item '{}', but got: {:?}",
                label,
                self.items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        }
        self
    }

    /// Assert that a completion item does NOT exist.
    ///
    /// # Panics
    ///
    /// Panics if an item with the label is found.
    #[must_use]
    pub fn expect_no_item(self, label: &str) -> Self {
        let found = self.items.iter().any(|item| item.label == label);
        assert!(
            !found,
            "Expected completion item '{}' to NOT exist, but it was found",
            label
        );
        self
    }

    /// Assert that a completion item has a specific kind.
    ///
    /// # Panics
    ///
    /// Panics if the item is not found or has a different kind.
    #[must_use]
    pub fn expect_item_kind(self, label: &str, kind: CompletionItemKind) -> Self {
        let item = self.items.iter().find(|item| item.label == label);
        let Some(item) = item else {
            panic!(
                "Completion item '{}' not found. Available: {:?}",
                label,
                self.items.iter().map(|i| &i.label).collect::<Vec<_>>()
            );
        };

        assert_eq!(
            item.kind,
            Some(kind),
            "Expected completion '{}' to have kind {:?}, but got {:?}",
            label,
            kind,
            item.kind
        );

        self
    }

    /// Assert the exact number of completions.
    ///
    /// # Panics
    ///
    /// Panics if the count doesn't match.
    #[must_use]
    pub fn expect_count(self, n: usize) -> Self {
        assert_eq!(
            self.items.len(),
            n,
            "Expected {} completions, but got {}:\n{}",
            n,
            self.items.len(),
            format_completions_snapshot(&self.items)
        );
        self
    }

    /// Snapshot test the completion list.
    ///
    /// Uses insta for snapshot comparison.
    #[must_use]
    pub fn assert_snapshot(self) -> LspTest {
        let formatted = format_completions_snapshot(&self.items);
        insta::assert_snapshot!(formatted);
        self.test
    }

    /// Finish and return to the test builder.
    #[must_use]
    pub fn done(self) -> LspTest {
        self.test
    }

    /// Get the raw completion items for custom assertions.
    #[must_use]
    pub fn raw(self) -> (LspTest, Vec<CompletionItem>) {
        (self.test, self.items)
    }
}
