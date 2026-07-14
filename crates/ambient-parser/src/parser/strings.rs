//! Interpolated-string parsing: `"a ${expr} b"` and its token-content
//! extraction helpers. Split from `expr.rs` to keep that file within its
//! line budget.

use ambient_engine::ast::Span;

use super::Parser;
use crate::cst::{CstExpr, CstExprKind, StringPart};
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::TokenKind;

impl Parser<'_> {
    pub(super) fn parse_interpolated_string(&mut self) -> Result<CstExpr, ParseError> {
        let start = self.current().span.start;
        let mut parts = Vec::new();

        // First part (StringStart) - token text includes leading `"` but not trailing `${`
        let token = self.advance();
        let content = Self::extract_string_start_content(&token.text);
        if !content.is_empty() {
            let unescaped = Self::unescape_string_part(content);
            parts.push(StringPart::Literal(unescaped.into(), token.span));
        }

        loop {
            // Parse interpolated expression
            let expr = self.parse_expression()?;
            parts.push(StringPart::Expr(expr));

            // Check what comes next
            match self.current_kind() {
                TokenKind::StringMiddle => {
                    // Token text includes leading `}` but not trailing `${`
                    let token = self.advance();
                    let content = Self::extract_string_middle_content(&token.text);
                    if !content.is_empty() {
                        let unescaped = Self::unescape_string_part(content);
                        parts.push(StringPart::Literal(unescaped.into(), token.span));
                    }
                }
                TokenKind::StringEnd => {
                    // Token text includes leading `}` and trailing `"`
                    let token = self.advance();
                    let content = Self::extract_string_end_content(&token.text);
                    if !content.is_empty() {
                        let unescaped = Self::unescape_string_part(content);
                        parts.push(StringPart::Literal(unescaped.into(), token.span));
                    }
                    break;
                }
                _ => {
                    return Err(ParseError::new(
                        ParseErrorKind::UnterminatedInterpolation,
                        self.current().span,
                    ));
                }
            }
        }

        let end = self.current().span.start;
        Ok(CstExpr {
            kind: CstExprKind::InterpolatedString(parts),
            span: Span::new(start, end),
        })
    }

    /// Extract content from `StringStart` token (strip leading `"`).
    fn extract_string_start_content(text: &str) -> &str {
        text.strip_prefix('"').unwrap_or(text)
    }

    /// Extract content from `StringMiddle` token (strip leading `}`).
    fn extract_string_middle_content(text: &str) -> &str {
        text.strip_prefix('}').unwrap_or(text)
    }

    /// Extract content from `StringEnd` token (strip leading `}` and trailing `"`).
    fn extract_string_end_content(text: &str) -> &str {
        text.strip_prefix('}')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(text)
    }
}
