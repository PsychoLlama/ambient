//! Code as text, for a report to show what a comptime call gave. See
//! "Unsettled" in `spec/comptime.md`.
//!
//! This is not the canonical format of `spec/trees.md`. It writes the
//! nodes a tree of code has; one no tree gives is written `…`. A local
//! has no name in a tree, so each is named by the order it was bound
//! in, which is the order resolution numbers them in.

mod page;

pub use self::page::Shown;
use self::page::{Doc, Page, list, text};
use super::{Arm, Expr, Function, Key, Operator, Pattern, Reference};
use crate::symbol::ModuleId;
use crate::types::{Ty, shown as type_shown};

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
    let generic = |(at, param): (usize, &super::TypeParam<Reference>)| match &param.bound {
        Some(bound) => format!("T{at}: {}", type_shown(bound)),
        None => format!("T{at}"),
    };
    let generic: Vec<String> = function
        .type_params
        .iter()
        .enumerate()
        .map(generic)
        .collect();
    let generic = match generic.as_slice() {
        [] => String::new(),
        generic => format!("<{}>", generic.join(", ")),
    };
    let mut header = vec![text(format!("fn {name}{generic}")), list("(", params, ")")];
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
            Expr::Cast { otherwise, .. } => !otherwise.is_settled(),
            Expr::Lambda { .. } | Expr::Match { .. } | Expr::Let { .. } => true,
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
            Expr::AbilityId(ability) => {
                text(format!("intrinsics.ability_id({})", self.name(ability)))
            }
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
                Doc::Row(vec![self.atom(target), text(self.member(key))])
            }
            Expr::Send {
                target,
                method,
                args,
            } => {
                let method = text(self.member(method));
                Doc::Row(vec![
                    self.atom(target),
                    method,
                    list("(", self.all(args), ")"),
                ])
            }
            Expr::Cast {
                value,
                ty,
                otherwise,
            } => self.cast(value, ty, otherwise),
            Expr::Operation { operator, operands } => self.operation(*operator, operands),
            Expr::Provide {
                bundle,
                restricted,
                body,
            } => {
                let provide = text(if *restricted {
                    "provide(restricted) "
                } else {
                    "provide "
                });
                let body = Doc::Body(Box::new(self.expr(body)));
                Doc::Row(vec![provide, self.atom(bundle), text(" =>"), body])
            }
            Expr::Merge { left, right } => {
                let under = Doc::Row(vec![text("..."), self.atom(left)]);
                let over = Doc::Row(vec![text("..."), self.atom(right)]);
                list("{ ", vec![under, over], " }")
            }
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

    /// A cast, whose other branch is taken only where the value does
    /// not pass. Written as a call it is evaluated first, which is the
    /// same where evaluating it does nothing; where it might, the cast
    /// is written as the match it is.
    fn cast(&mut self, value: &'a Expr<Reference>, ty: &Ty, otherwise: &'a Expr<Reference>) -> Doc {
        let ty = type_shown(ty);
        if otherwise.is_settled() {
            let given = vec![self.expr(value), self.expr(otherwise)];
            return Doc::Row(vec![text(format!("cast::<{ty}>")), list("(", given, ")")]);
        }
        let subject = self.atom(value);
        let passes = text(format!("let passed: {ty} => passed,"));
        let fails = Doc::Row(vec![
            text("_ =>"),
            Doc::Body(Box::new(self.expr(otherwise))),
            text(","),
        ]);
        let arms = Doc::Block(vec![passes, fails]);
        Doc::Row(vec![text("match "), subject, text(" "), arms])
    }

    /// An operation: between its operands if it has an operator, and
    /// by its word if not.
    fn operation(&mut self, operator: Operator, operands: &'a [Expr<Reference>]) -> Doc {
        if let Some((mark, left, right)) = infix(operator, operands) {
            let operator = text(format!(" {mark} "));
            return Doc::Row(vec![self.atom(left), operator, self.atom(right)]);
        }
        let word = text(format!("intrinsics.{}", operator.name()));
        Doc::Row(vec![word, list("(", self.all(operands), ")")])
    }

    fn key(&self, key: &Key<Reference>) -> String {
        match key {
            Key::Field(name) => name.clone(),
            Key::Ability(ability) => format!("[{}]", self.name(ability)),
        }
    }

    /// A key after what holds it: `.name`, or `[ability]`.
    fn member(&self, key: &Key<Reference>) -> String {
        match key {
            Key::Field(name) => format!(".{name}"),
            Key::Ability(_) => self.key(key),
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
) -> Option<(&'static str, &Expr<Reference>, &Expr<Reference>)> {
    match operands {
        [left, right] => Some((operator.mark()?, left, right)),
        _ => None,
    }
}
