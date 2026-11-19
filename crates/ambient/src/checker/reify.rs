//! Trees as what they stand for: the type a `tree.Type` a comptime call
//! gave is, and the definition a `tree.Reference` refers to. See
//! "Splice positions" and "Trees as values" in `docs/spec/comptime.md`.
//!
//! This reads what [`super::reflect`] writes, from the literal a
//! comptime result was spliced as. [`super::code`] reads expressions
//! and [`super::definitions`] functions and modules, with what is
//! here.
//!
//! A literal read here was checked against the type of its tree, so
//! its shape is right. What may still be wrong is what a type cannot
//! say: a local that nothing binds, a reference to the wrong kind of
//! definition, a name no definition could have. Each is said in
//! words, for the author of the function that gave the tree.

use ambient_runtime::FAULT_PACKAGE;

use crate::core::{Expr, Key};
use crate::literal::{Brand, Literal};
use crate::surface::{self, Reference};
use crate::symbol::{ModuleId, Namespace, Symbol};
use crate::types::Ty;

/// The module of the library that says what a tree is.
pub(super) const MODULE: &str = "tree";

/// The type of a type's tree: what a parameter has that is handed a
/// type, and what a call gives that a type is derived from.
#[must_use]
pub fn tree() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Type")
}

/// The type of a reference: what a parameter has that is handed the
/// definition its argument names.
#[must_use]
pub fn reference() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Reference")
}

/// The module a tree is the content of, if it is that of one: where a
/// definition it names as beside it is.
pub(super) type Home<'a> = Option<&'a ModuleId>;

/// What a tree stands for, or why it stands for nothing.
pub type Read<T> = Result<T, String>;

/// What is wrong with a literal that is not shaped as its tree's type
/// says. Checking the literal against that type rules it out, so this
/// is said only if the two have come apart.
fn shapeless<T>(what: &str) -> Read<T> {
    Err(format!("{what} is not shaped as one is"))
}

/// The type of an expression's tree: what a function gives for its
/// result to be spliced as code.
#[must_use]
pub fn code() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Expr")
}

/// The type of a function's tree: what a call gives that a function
/// is generated from.
#[must_use]
pub fn function() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Function")
}

/// The type of a module's tree: what a call gives that is the whole
/// content of a module.
#[must_use]
pub fn module() -> Symbol {
    ModuleId::new(FAULT_PACKAGE, MODULE).ty("Module")
}

/// The fields of a record that was spliced.
pub(super) fn fields<'a>(tree: &'a Expr, what: &str) -> Read<&'a [(Key, Expr)]> {
    match tree {
        Expr::Record(fields) => Ok(fields),
        _ => shapeless(what),
    }
}

/// The field `name` of a record that was spliced, if it has one.
pub(super) fn field<'a>(fields: &'a [(Key, Expr)], name: &str) -> Option<&'a Expr> {
    let named = |(key, _): &&(Key, Expr)| matches!(key, Key::Field(field) if field == name);
    fields.iter().find(named).map(|(_, value)| value)
}

/// The field `name` of a record that was spliced, which has one.
pub(super) fn need<'a>(fields: &'a [(Key, Expr)], name: &str) -> Read<&'a Expr> {
    field(fields, name).ok_or_else(|| format!("a tree has no `{name}`"))
}

pub(super) fn text(expr: &Expr) -> Read<&str> {
    match expr {
        Expr::Literal(Literal::String(text)) => Ok(text),
        _ => shapeless("a text"),
    }
}

pub(super) fn each<T>(list: &Expr, read: impl FnMut(&Expr) -> Read<T>) -> Read<Vec<T>> {
    match list {
        Expr::List(members) => members.iter().map(read).collect(),
        _ => shapeless("a list"),
    }
}

/// The type a spliced `tree.Type` stands for, which is inside no
/// definition: one that names no type parameter.
///
/// # Errors
///
/// Fails, saying why, if it stands for none.
pub fn closed(tree: &Expr) -> Read<Ty> {
    let ty = written(None, tree)?;
    let mut within = Ok(());
    ty.names(&mut |name| within = within.clone().and(parameter_within(0, name)));
    within.map(|()| ty)
}

/// Fails if `name` is a type parameter and not one of the first
/// `count`, which are all the definition it is in has.
pub(super) fn parameter_within(count: usize, name: &Reference) -> Read<()> {
    match name {
        Reference::Parameter(at) if *at >= count => Err(format!(
            "type parameter {at} is named where there are {count}"
        )),
        _ => Ok(()),
    }
}

/// The type a spliced `tree.Type` stands for, which may name type
/// parameters: whatever reads the definition it is in says how many
/// there are.
pub(super) fn written(home: Home, tree: &Expr) -> Read<Ty> {
    match tree {
        Expr::Literal(Literal::Tag { package, tag }) if package == FAULT_PACKAGE => {
            let name = tag
                .strip_prefix(MODULE)
                .and_then(|name| name.strip_prefix('.'));
            name.and_then(primitive)
                .ok_or_else(|| format!("the tag `{tag}` is no type"))
        }
        Expr::Record(fields) => record(home, fields),
        _ => shapeless("a type"),
    }
}

fn primitive(name: &str) -> Option<Ty> {
    Some(match name {
        "none" => Ty::Never,
        "any" => Ty::Unknown,
        "nil" => Ty::Null,
        "boolean" => Ty::Bool,
        "integer" => Ty::Int,
        "number" => Ty::Float,
        "text" => Ty::String,
        "binary" => Ty::Bytes,
        "tagged" => Ty::Tag,
        _ => return None,
    })
}

/// The definition a spliced `tree.Reference` refers to, which must be
/// of one of the namespaces `kinds` name: a reference says what kind
/// of definition it is to, and each place in a tree takes only some.
/// One to a definition beside the tree is to one of `home`.
pub(super) fn definition(home: Home, reference: &Expr, kinds: &[&str]) -> Read<Reference> {
    let fields = fields(reference, "a reference")?;
    let text = |name: &str| need(fields, name).and_then(text);
    let (kind, name) = (text("kind")?, text("name")?);
    let namespace = match kind {
        "type" => Namespace::Type,
        "value" => Namespace::Value,
        "ability" => Namespace::Ability,
        "contract" => Namespace::Contract,
        _ => return shapeless("a reference"),
    };
    if !kinds.contains(&kind) {
        let wanted = kinds.join("` or `");
        return Err(format!(
            "the reference to `{name}` is of kind `{kind}`, where `{wanted}` is taken"
        ));
    }
    if field(fields, "beside").is_some() {
        let home = home.ok_or_else(|| format!("`{name}` is beside a tree that is no module"))?;
        return Ok(Reference::Definition(home.symbol(namespace, name)));
    }
    let path = each(need(fields, "module")?, text_of)?;
    let module = ModuleId::new(text("package")?, &path.join("."));
    Ok(Reference::Definition(module.symbol(namespace, name)))
}

fn text_of(expr: &Expr) -> Read<String> {
    text(expr).map(str::to_owned)
}

/// The ability a spliced `tree.Ability` is.
fn taken(home: Home, ability: &Expr) -> Read<Reference> {
    let fields = fields(ability, "an ability")?;
    definition(home, need(fields, "ability")?, &["ability"])
}

/// The brand a spliced `tree.Brand` is.
fn brand(brand: &Expr) -> Read<Ty> {
    let fields = fields(brand, "a brand")?;
    let text = |name: &str| need(fields, name).and_then(text_of);
    Ok(Ty::Brand(Brand {
        package: text("package")?,
        brand: text("identity")?,
    }))
}

fn record(home: Home, fields: &[(Key, Expr)]) -> Read<Ty> {
    let need = |name: &str| need(fields, name);
    let written = |tree: &Expr| written(home, tree);
    let boxed = |tree: &Expr| written(tree).map(Box::new);
    Ok(match fields {
        [(Key::Field(name), holds)] => match (name.as_str(), holds) {
            ("literal", Expr::Literal(literal)) => Ty::Literal(literal.clone()),
            ("list", member) => Ty::List(boxed(member)?),
            ("mutable", held) => Ty::Cell(boxed(held)?),
            ("iterator", member) => Ty::Iterator(boxed(member)?),
            ("tuple", members) => Ty::Tuple(each(members, written)?.into()),
            ("union", members) => Ty::Union(each(members, written)?),
            ("named", named) => Ty::Named(definition(home, named, &["type"])?, Vec::new()),
            ("parameter", Expr::Literal(Literal::Int(at))) => {
                let at = usize::try_from(*at).map_err(|_| format!("{at} is no type parameter"))?;
                Ty::Named(Reference::Parameter(at), Vec::new())
            }
            ("record", members) => Ty::Record(each(members, |field| member(home, field))?),
            _ => return shapeless("a type"),
        },
        _ if field(fields, "applied").is_some() => {
            let applied = definition(home, need("applied")?, &["type"])?;
            Ty::Named(applied, each(need("to")?, written)?)
        }
        _ if field(fields, "brands").is_some() => {
            let brands = each(need("brands")?, brand)?;
            match (need("under")?, brands.as_slice()) {
                (Expr::Null, [handle]) => handle.clone(),
                (Expr::Null, _) => {
                    return Err(format!(
                        "a handle has one brand, and this has {}",
                        brands.len()
                    ));
                }
                (under, _) => Ty::Branded(boxed(under)?, brands),
            }
        }
        _ => Ty::Function {
            params: each(need("function")?, written)?.into(),
            ret: boxed(need("returns")?)?,
            row: each(need("takes")?, |ability| taken(home, ability))?,
        },
    })
}

/// A field of a record type, from its `tree.Field`.
fn member(home: Home, field: &Expr) -> Read<(surface::Key<Reference>, Ty)> {
    let fields = fields(field, "a field")?;
    let name = text(need(fields, "name")?)?;
    let holds = written(home, need(fields, "holds")?)?;
    Ok((surface::Key::Field(name.to_owned()), holds))
}
