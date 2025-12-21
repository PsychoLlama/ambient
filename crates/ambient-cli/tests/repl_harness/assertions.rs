//! Assertion result types for fluent chained assertions.
//!
//! These types allow chaining multiple assertions on specific results,
//! following the pattern established by the LSP test harness.

#![allow(dead_code)]

use super::builder::ReplTest;

/// Result from checking the current line content.
pub struct LineResult {
    /// The test context.
    pub(crate) test: ReplTest,
    /// The current line content.
    pub(crate) line: String,
}

impl LineResult {
    /// Assert the line contains the expected text.
    #[must_use]
    pub fn contains(self, expected: &str) -> Self {
        assert!(
            self.line.contains(expected),
            "Expected line to contain '{}', but got: '{}'",
            expected,
            self.line
        );
        self
    }

    /// Assert the line equals the expected text exactly.
    #[must_use]
    pub fn equals(self, expected: &str) -> Self {
        assert_eq!(
            self.line, expected,
            "Expected line '{}', but got: '{}'",
            expected, self.line
        );
        self
    }

    /// Assert the line starts with the expected prefix.
    #[must_use]
    pub fn starts_with(self, prefix: &str) -> Self {
        assert!(
            self.line.starts_with(prefix),
            "Expected line to start with '{}', but got: '{}'",
            prefix,
            self.line
        );
        self
    }

    /// Assert the line ends with the expected suffix.
    #[must_use]
    pub fn ends_with(self, suffix: &str) -> Self {
        assert!(
            self.line.ends_with(suffix),
            "Expected line to end with '{}', but got: '{}'",
            suffix,
            self.line
        );
        self
    }

    /// Get the raw line for custom assertions.
    pub fn raw(&self) -> &str {
        &self.line
    }

    /// Continue testing without additional line assertions.
    #[must_use]
    pub fn done(self) -> ReplTest {
        self.test
    }
}

/// Result from checking output content.
pub struct OutputResult {
    /// The test context.
    pub(crate) test: ReplTest,
    /// The full output content.
    pub(crate) output: String,
}

impl OutputResult {
    /// Assert the output contains the expected text.
    #[must_use]
    pub fn contains(self, expected: &str) -> Self {
        assert!(
            self.output.contains(expected),
            "Expected output to contain '{}', but output was:\n{}",
            expected,
            self.output
        );
        self
    }

    /// Assert the output contains all the expected texts.
    #[must_use]
    pub fn contains_all(self, expected: &[&str]) -> Self {
        for text in expected {
            assert!(
                self.output.contains(text),
                "Expected output to contain '{}', but output was:\n{}",
                text,
                self.output
            );
        }
        self
    }

    /// Assert the output does NOT contain the text.
    #[must_use]
    pub fn not_contains(self, unexpected: &str) -> Self {
        assert!(
            !self.output.contains(unexpected),
            "Expected output to NOT contain '{}', but it did:\n{}",
            unexpected,
            self.output
        );
        self
    }

    /// Get the raw output for custom assertions.
    pub fn raw(&self) -> &str {
        &self.output
    }

    /// Continue testing without additional output assertions.
    #[must_use]
    pub fn done(self) -> ReplTest {
        self.test
    }
}
