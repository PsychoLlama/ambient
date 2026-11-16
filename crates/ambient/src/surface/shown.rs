//! Code as text, for a report to show what a comptime call gave. See
//! "Unsettled" in `spec/comptime.md`.
//!
//! This is not the canonical format of `spec/trees.md`. It writes the
//! nodes a tree of code has; one no tree gives is written `…`. A local
//! has no name in a tree, so each is named by the order it was bound
//! in, which is the order resolution numbers them in.

use std::ops::Range;

use super::{Arm, Expr, Function, Key, Operator, Pattern, Reference};
use crate::symbol::ModuleId;
use crate::types::{Ty, shown as type_shown};

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

/// A function a call gave, written as the definition `name` of the
/// module `home`, with the node of its body numbered `node` marked.
#[must_use]
pub fn function(
    name: &str,
    home: &ModuleId,
    function: &Function<Reference>,
    node: Option<usize>,
) -> Shown {
    let marked = node.and_then(|node| function.body.nth(node));
    let mut writer = Writer::new(home, 0, marked);
    let params = function.params.iter().map(|param| writer.param(&param.ty));
    let params = params.collect();
    let mut header = vec![text(format!("fn {name}")), list("(", params, ")")];
    header.push(text(format!(": {}", type_shown(&function.ret))));
    if !function.row.is_empty() {
        let row: Vec<String> = function.row.iter().map(|name| writer.name(name)).collect();
        header.push(text(format!(" with {}", row.join(", "))));
    }
    header.push(text(" =>"));
    header.push(Doc::Body(Box::new(writer.expr(&function.body))));
    Page::of(&Doc::Row(header))
}

/// An expression a call gave to be spliced in a definition of the
/// module `home`, whose locals are numbered from `base`, with `marked`
/// marked if it is a node of it.
#[must_use]
pub fn expression(
    home: &ModuleId,
    base: usize,
    code: &Expr<Reference>,
    marked: Option<*const Expr<Reference>>,
) -> Shown {
    let marked = marked.and_then(|node| code.nth(code.index_of(node)?));
    Page::of(&Writer::new(home, base, marked).expr(code))
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
enum Doc {
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

fn text(text: impl Into<String>) -> Doc {
    Doc::Text(text.into())
}

fn list(open: &'static str, items: Vec<Doc>, close: &'static str) -> Doc {
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
struct Page {
    out: String,
    marked: Option<Range<usize>>,
}

impl Page {
    fn of(doc: &Doc) -> Shown {
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

/// What writes code as a [`Doc`], naming each local as it is bound.
struct Writer<'a> {
    home: &'a ModuleId,
    /// The number of the first local the code binds.
    base: usize,
    /// The number the next binder is given.
    next: usize,
    marked: Option<&'a Expr<Reference>>,
}

impl<'a> Writer<'a> {
    fn new(home: &'a ModuleId, base: usize, marked: Option<&'a Expr<Reference>>) -> Self {
        let next = base;
        Self {
            home,
            base,
            next,
            marked,
        }
    }

    /// The name of the local numbered `number`.
    fn local(&self, number: usize) -> String {
        match number.checked_sub(self.base) {
            Some(place) => format!("_{place}"),
            None => "_".to_owned(),
        }
    }

    /// The name of the local the next binder binds.
    fn bind(&mut self) -> String {
        self.next += 1;
        self.local(self.next - 1)
    }

    fn param(&mut self, ty: &Ty) -> Doc {
        text(format!("{}: {}", self.bind(), type_shown(ty)))
    }

    /// A name: a definition of the code's own module by its name
    /// alone, and any other by its module too.
    fn name(&self, name: &Reference) -> String {
        match name {
            Reference::Local(number) => self.local(*number),
            Reference::Parameter(_) => "_".to_owned(),
            Reference::Definition(symbol) if symbol.module == *self.home => symbol.name.clone(),
            Reference::Definition(symbol) => symbol.to_string(),
        }
    }

    fn all(&mut self, exprs: &'a [Expr<Reference>]) -> Vec<Doc> {
        exprs.iter().map(|expr| self.expr(expr)).collect()
    }

    /// An expression where only something that holds together may be:
    /// one that would not is put in brackets.
    fn atom(&mut self, expr: &'a Expr<Reference>) -> Doc {
        let loose = match expr {
            Expr::Operation { operator, operands } => infix(*operator, operands).is_some(),
            Expr::Cast { .. } | Expr::Lambda { .. } | Expr::Match { .. } | Expr::Let { .. } => true,
            _ => false,
        };
        let doc = self.expr(expr);
        if loose {
            Doc::Row(vec![text("("), doc, text(")")])
        } else {
            doc
        }
    }

    fn expr(&mut self, expr: &'a Expr<Reference>) -> Doc {
        let doc = self.node(expr);
        match self.marked {
            Some(marked) if std::ptr::eq(marked, expr) => Doc::Marked(Box::new(doc)),
            _ => doc,
        }
    }

    fn node(&mut self, expr: &'a Expr<Reference>) -> Doc {
        match expr {
            Expr::Null => text("null"),
            Expr::Literal(literal) => text(literal.to_string()),
            Expr::Name(name) => text(self.name(name)),
            Expr::AbilityId(ability) => text(format!("@ability_id({})", self.name(ability))),
            Expr::List(items) => list("[", self.all(items), "]"),
            Expr::Tuple(members) => list("(", self.all(members), ")"),
            Expr::Record(fields) => {
                let field = |(key, value): &'a (Key<Reference>, Expr<Reference>)| {
                    Doc::Row(vec![
                        text(format!("{}:", self.key(key))),
                        Doc::Body(Box::new(self.expr(value))),
                    ])
                };
                list("{ ", fields.iter().map(field).collect(), " }")
            }
            Expr::Call { callee, args } => {
                Doc::Row(vec![self.atom(callee), list("(", self.all(args), ")")])
            }
            Expr::Perform { ability, args } => {
                let ability = text(format!("{}!", self.name(ability)));
                Doc::Row(vec![ability, list("(", self.all(args), ")")])
            }
            Expr::Field { target, key } => {
                let key = match key {
                    Key::Field(name) => name.clone(),
                    Key::Ability(_) => self.key(key),
                };
                Doc::Row(vec![self.atom(target), text(format!(".{key}"))])
            }
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => {
                let to = text(format!(" as {} else ", type_shown(ty)));
                Doc::Row(vec![self.atom(value), to, self.atom(otherwise)])
            }
            Expr::Operation { operator, operands } => self.operation(*operator, operands),
            Expr::Let { .. } => self.block(expr),
            Expr::Match { subject, arms } => {
                let subject = self.atom(subject);
                let arms = arms.iter().map(|arm| self.arm(arm)).collect();
                Doc::Row(vec![text("match "), subject, text(" "), Doc::Block(arms)])
            }
            Expr::Lambda { params, body, .. } => {
                let param = |(_, ty): &(String, Option<Ty>)| match ty {
                    Some(ty) => self.param(ty),
                    None => text(self.bind()),
                };
                let params = params.iter().map(param).collect();
                let body = Doc::Body(Box::new(self.expr(body)));
                Doc::Row(vec![text("fn"), list("(", params, ")"), text(" =>"), body])
            }
            _ => text("…"),
        }
    }

    /// An operation: between its operands if it has an operator, and
    /// by its word if not.
    fn operation(&mut self, operator: Operator, operands: &'a [Expr<Reference>]) -> Doc {
        if let Some((left, right)) = infix(operator, operands) {
            let operator = text(format!(" {} ", operator.written()));
            return Doc::Row(vec![self.atom(left), operator, self.atom(right)]);
        }
        let word = text(format!("@{}", operator.name()));
        Doc::Row(vec![word, list("(", self.all(operands), ")")])
    }

    fn key(&self, key: &Key<Reference>) -> String {
        match key {
            Key::Field(name) => name.clone(),
            Key::Ability(ability) => format!("[{}]", self.name(ability)),
        }
    }

    /// Values bound one inside another, as the lines of a block: each
    /// binding, and then what the innermost gives.
    fn block(&mut self, mut expr: &'a Expr<Reference>) -> Doc {
        let mut lines = Vec::new();
        while let Expr::Let { value, body, .. } = expr {
            // A value is outside what it is bound as.
            let value = Doc::Body(Box::new(self.expr(value)));
            let bound = text(format!("let {} =", self.bind()));
            lines.push(Doc::Row(vec![bound, value, text(";")]));
            expr = body;
        }
        lines.push(self.expr(expr));
        Doc::Block(lines)
    }

    fn arm(&mut self, arm: &'a Arm<Reference>) -> Doc {
        let pattern = text(format!("{} =>", self.pattern(&arm.pattern)));
        let body = Doc::Body(Box::new(self.expr(&arm.body)));
        Doc::Row(vec![pattern, body, text(",")])
    }

    fn pattern(&mut self, pattern: &Pattern<Reference>) -> String {
        let anything = |pattern: &Pattern<Reference>| {
            matches!(pattern, Pattern::Wildcard | Pattern::Is(Ty::Unknown))
        };
        match pattern {
            Pattern::Wildcard | Pattern::Is(Ty::Unknown) => "_".to_owned(),
            Pattern::Literal(literal) => literal.to_string(),
            Pattern::Is(ty) => type_shown(ty),
            Pattern::Bind(_, inner) => {
                // A binding is numbered before those inside it.
                let bound = format!("let {}", self.bind());
                if anything(inner) {
                    bound
                } else {
                    format!("{bound}: {}", self.pattern(inner))
                }
            }
            Pattern::Tuple(members) => {
                let members: Vec<String> =
                    members.iter().map(|member| self.pattern(member)).collect();
                format!("({})", members.join(", "))
            }
            Pattern::Record(fields) => {
                let field = |(name, pattern): &(String, Pattern<Reference>)| {
                    format!("{name}: {}", self.pattern(pattern))
                };
                let fields: Vec<String> = fields.iter().map(field).collect();
                format!("{{ {} }}", fields.join(", "))
            }
        }
    }
}

/// The two operands of an operation that is written between them.
fn infix(
    operator: Operator,
    operands: &[Expr<Reference>],
) -> Option<(&Expr<Reference>, &Expr<Reference>)> {
    match operands {
        [left, right] if !operator.written().starts_with('@') => Some((left, right)),
        _ => None,
    }
}
