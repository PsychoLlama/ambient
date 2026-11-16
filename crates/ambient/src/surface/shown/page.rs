//! Text laid out on a page: what is too wide for a line is broken
//! over several, and one part of it can be found again to be marked.

use std::ops::Range;

/// How wide a line may be before what is on it is broken over several.
const WIDTH: usize = 80;
const INDENT: usize = 4;
/// How many lines are shown on each side of the one that is marked.
const AROUND: usize = 3;

/// Code written out, and where in the text one node of it is.
pub struct Shown {
    pub text: String,
    pub marked: Option<Range<usize>>,
}

impl Shown {
    /// The lines around the marked node, with the node underlined; or
    /// the first lines, if none is marked. The first line is always
    /// there: it says what the code is.
    #[must_use]
    pub fn excerpt(&self) -> String {
        let lines: Vec<&str> = self.text.lines().collect();
        let Some(marked) = &self.marked else {
            return excerpt(&lines, 0..lines.len().min(2 * AROUND + 1), None);
        };
        let before = &self.text[..marked.start];
        let line = before.matches('\n').count();
        let column = before.chars().rev().take_while(|c| *c != '\n').count();
        // A node of several lines is underlined on its first.
        let length = self.text[marked.clone()]
            .lines()
            .next()
            .map_or(0, |first| first.chars().count());
        let shown = line.saturating_sub(AROUND)..lines.len().min(line + AROUND + 1);
        excerpt(&lines, shown, Some((line, column, length.max(1))))
    }
}

/// The lines numbered `shown` of `lines`, after the first, with `…`
/// where lines are left out and carets under what is marked: a line,
/// a column, and a length.
fn excerpt(lines: &[&str], shown: Range<usize>, marked: Option<(usize, usize, usize)>) -> String {
    let margin = " ".repeat(INDENT);
    let mut out = String::new();
    let mut put = |line: &str| {
        out.push_str(&margin);
        out.push_str(line);
        out.push('\n');
    };
    if shown.start > 0 {
        put(lines[0]);
    }
    if shown.start > 1 {
        put("…");
    }
    for at in shown.clone() {
        put(lines[at]);
        if let Some((line, column, length)) = marked
            && line == at
        {
            put(&format!("{}{}", " ".repeat(column), "^".repeat(length)));
        }
    }
    if shown.end < lines.len() {
        put("…");
    }
    out
}

/// Text that is not yet on a page: what may be broken over lines says
/// how.
pub(super) enum Doc {
    Text(String),
    /// One after another.
    Row(Vec<Doc>),
    /// Items between two marks, with commas: on one line if they fit,
    /// and on a line each if not.
    List(&'static str, Vec<Doc>, &'static str),
    /// What follows a `=>` or an `=`: on the same line if its first
    /// line fits there, and on the next if not.
    Body(Box<Doc>),
    /// Lines between braces, never on one line.
    Block(Vec<Doc>),
    /// The node to mark.
    Marked(Box<Doc>),
}

pub(super) fn text(text: impl Into<String>) -> Doc {
    Doc::Text(text.into())
}

pub(super) fn list(open: &'static str, items: Vec<Doc>, close: &'static str) -> Doc {
    Doc::List(open, items, close)
}

impl Doc {
    /// How wide it is on one line; more than any line, if it cannot
    /// be on one.
    fn width(&self) -> usize {
        let sum = |docs: &[Self]| {
            let widths = docs.iter().map(Self::width);
            widths.fold(0, usize::saturating_add)
        };
        match self {
            Self::Text(text) => text.chars().count(),
            Self::Row(parts) => sum(parts),
            Self::List(open, items, close) => {
                let commas = 2 * items.len().saturating_sub(1);
                sum(items).saturating_add(open.len() + close.len() + commas)
            }
            Self::Body(body) => body.width().saturating_add(1),
            Self::Block(_) => usize::MAX,
            Self::Marked(inner) => inner.width(),
        }
    }
}

impl Doc {
    /// How wide its first line is, if it is on as few as it can be:
    /// a block's is its opening brace.
    fn first(&self) -> usize {
        match self {
            Self::Block(_) => 1,
            Self::Row(parts) => {
                let mut wide = 0_usize;
                for part in parts {
                    wide = wide.saturating_add(part.first());
                    if part.width() == usize::MAX {
                        break;
                    }
                }
                wide
            }
            Self::Marked(inner) => inner.first(),
            _ => self.width(),
        }
    }
}

/// The text so far.
pub(super) struct Page {
    out: String,
    marked: Option<Range<usize>>,
}

impl Page {
    pub(super) fn of(doc: &Doc) -> Shown {
        let mut page = Self {
            out: String::new(),
            marked: None,
        };
        page.write(doc, 0);
        Shown {
            text: page.out,
            marked: page.marked,
        }
    }

    /// Whether `wide` more columns fit on the line being written.
    fn fits(&self, wide: usize) -> bool {
        let column = self.out.chars().rev().take_while(|c| *c != '\n').count();
        column.saturating_add(wide) <= WIDTH
    }

    fn line(&mut self, indent: usize) {
        self.out.push('\n');
        self.out.push_str(&" ".repeat(indent));
    }

    fn write(&mut self, doc: &Doc, indent: usize) {
        let inner = indent + INDENT;
        match doc {
            Doc::Text(text) => self.out.push_str(text),
            Doc::Row(parts) => parts.iter().for_each(|part| self.write(part, indent)),
            Doc::Marked(marked) => {
                let from = self.out.len();
                self.write(marked, indent);
                self.marked = Some(from..self.out.len());
            }
            Doc::List(open, items, close) if self.fits(doc.width()) => {
                self.out.push_str(open);
                for (at, item) in items.iter().enumerate() {
                    self.out.push_str(if at > 0 { ", " } else { "" });
                    self.write(item, indent);
                }
                self.out.push_str(close);
            }
            Doc::List(open, items, close) => {
                self.out.push_str(open.trim_end());
                for item in items {
                    self.line(inner);
                    self.write(item, inner);
                    self.out.push(',');
                }
                self.line(indent);
                self.out.push_str(close.trim_start());
            }
            Doc::Body(body) if self.fits(body.first().saturating_add(1)) => {
                self.out.push(' ');
                self.write(body, indent);
            }
            Doc::Body(body) => {
                self.line(inner);
                self.write(body, inner);
            }
            Doc::Block(lines) => {
                self.out.push('{');
                for line in lines {
                    self.line(inner);
                    self.write(line, inner);
                }
                self.line(indent);
                self.out.push('}');
            }
        }
    }
}
