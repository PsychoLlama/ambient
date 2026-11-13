//! Patterns and expressions.

use super::{Sink, encode_literal, encode_slots, encode_type, encode_types, text, unsigned};
use crate::core::{Expr, Handler, Key, Pattern};

fn encode_pattern(pattern: &Pattern, out: &mut Sink<'_>) {
    match pattern {
        Pattern::Wildcard => out.push(0),
        Pattern::Literal(literal) => {
            out.push(1);
            encode_literal(literal, out);
        }
        Pattern::Record(fields) => {
            out.push(2);
            unsigned(fields.len() as u64, out);
            for (name, pattern) in fields {
                text(name, out);
                encode_pattern(pattern, out);
            }
        }
        Pattern::Tuple(members) => {
            out.push(3);
            unsigned(members.len() as u64, out);
            for member in members {
                encode_pattern(member, out);
            }
        }
        Pattern::Is(ty) => {
            out.push(4);
            encode_type(ty, out);
        }
        Pattern::Bind(inner) => {
            out.push(5);
            encode_pattern(inner, out);
        }
    }
}

fn encode_exprs(exprs: &[Expr], out: &mut Sink<'_>) {
    unsigned(exprs.len() as u64, out);
    for expr in exprs {
        encode_expr(expr, out);
    }
}

fn encode_keys(keys: &[Key], out: &mut Sink<'_>) {
    unsigned(keys.len() as u64, out);
    for key in keys {
        encode_key(key, out);
    }
}

pub(super) fn encode_expr(expr: &Expr, out: &mut Sink<'_>) {
    match expr {
        Expr::Null => out.push(0),
        Expr::Literal(literal) => {
            out.push(1);
            encode_literal(literal, out);
        }
        Expr::Local(index) => {
            out.push(2);
            unsigned(*index as u64, out);
        }
        Expr::Reference(slot, args) => {
            out.push(3);
            out.slot(*slot);
            encode_types(args, out);
        }
        Expr::Call { callee, args } => {
            out.push(4);
            encode_expr(callee, out);
            encode_exprs(args, out);
        }
        Expr::Record(fields) => {
            out.push(5);
            encode_fields(fields, out);
        }
        Expr::Tuple(members) => {
            out.push(6);
            encode_exprs(members, out);
        }
        Expr::List(members) => {
            out.push(7);
            encode_exprs(members, out);
        }
        Expr::Field { .. } | Expr::Send { .. } | Expr::Merge { .. } => encode_record(expr, out),
        Expr::Let { value, body } => {
            out.push(9);
            encode_expr(value, out);
            encode_expr(body, out);
        }
        Expr::Primitive { operation, args } => {
            out.push(10);
            out.push(*operation as u8);
            encode_exprs(args, out);
        }
        Expr::Match { subject, arms } => {
            out.push(11);
            encode_expr(subject, out);
            encode_arms(arms, out);
        }
        Expr::Cast {
            value,
            ty,
            otherwise,
        } => {
            out.push(12);
            encode_expr(value, out);
            encode_type(ty, out);
            encode_expr(otherwise, out);
        }
        Expr::Captured(index) => {
            out.push(13);
            unsigned(*index as u64, out);
        }
        Expr::Cell { ty, value } => {
            out.push(26);
            encode_type(ty, out);
            encode_expr(value, out);
        }
        Expr::Closure { .. }
        | Expr::Method(_)
        | Expr::Instantiate { .. }
        | Expr::Step(_)
        | Expr::Perform { .. }
        | Expr::Provide { .. }
        | Expr::AbilityId(_)
        | Expr::All(_)
        | Expr::First(_)
        | Expr::Cleanup { .. }
        | Expr::Group(_)
        | Expr::Boundary(_)
        | Expr::Budget { .. }
        | Expr::Spawn(_) => {
            encode_binding(expr, out);
        }
    }
}

/// The nodes that read a record or make one of others.
fn encode_record(expr: &Expr, out: &mut Sink<'_>) {
    match expr {
        Expr::Field { target, key } => {
            out.push(8);
            encode_expr(target, out);
            encode_key(key, out);
        }
        Expr::Send {
            target,
            key,
            args,
            types,
        } => {
            out.push(30);
            encode_expr(target, out);
            encode_key(key, out);
            encode_exprs(args, out);
            encode_types(types, out);
        }
        Expr::Merge {
            left,
            right,
            keys,
            methods,
        } => {
            out.push(17);
            encode_expr(left, out);
            encode_expr(right, out);
            encode_keys(keys, out);
            encode_keys(methods, out);
        }
        _ => {}
    }
}

/// The nodes that have to do with closures and abilities.
fn encode_binding(expr: &Expr, out: &mut Sink<'_>) {
    match expr {
        Expr::Closure {
            slot,
            type_args,
            captures,
            bindings,
        } => {
            out.push(14);
            out.slot(*slot);
            encode_types(type_args, out);
            encode_exprs(captures, out);
            encode_slots(bindings, out);
        }
        Expr::Method(closure) => {
            out.push(29);
            encode_expr(closure, out);
        }
        Expr::Step(iterator) => {
            out.push(31);
            encode_expr(iterator, out);
        }
        Expr::Instantiate { function, types } => {
            out.push(32);
            encode_expr(function, out);
            encode_types(types, out);
        }
        Expr::AbilityId(ability) => {
            out.push(18);
            out.slot(*ability);
        }
        Expr::All(thunks) => {
            out.push(19);
            encode_exprs(thunks, out);
        }
        Expr::First(thunks) => {
            out.push(20);
            encode_exprs(thunks, out);
        }
        Expr::Budget {
            steps,
            allocation,
            body,
        } => {
            out.push(25);
            encode_expr(steps, out);
            encode_expr(allocation, out);
            encode_expr(body, out);
        }
        Expr::Boundary(body) => {
            out.push(24);
            encode_expr(body, out);
        }
        Expr::Group(body) => {
            out.push(22);
            encode_expr(body, out);
        }
        Expr::Spawn(thunk) => {
            out.push(23);
            encode_expr(thunk, out);
        }
        Expr::Cleanup { cleanup, body } => {
            out.push(21);
            encode_expr(cleanup, out);
            encode_expr(body, out);
        }
        Expr::Perform {
            ability,
            args,
            returns,
        } => {
            out.push(15);
            out.slot(*ability);
            encode_exprs(args, out);
            out.push(u8::from(*returns));
        }
        Expr::Provide {
            handlers,
            only,
            body,
        } => {
            out.push(16);
            out.push(u8::from(*only));
            encode_handlers(handlers, out);
            encode_expr(body, out);
        }
        _ => {}
    }
}

pub(super) fn encode_key(key: &Key, out: &mut Sink<'_>) {
    match key {
        Key::Field(name) => {
            out.push(0);
            text(name, out);
        }
        Key::Ability(slot) => {
            out.push(1);
            out.slot(*slot);
        }
    }
}

fn encode_fields(fields: &[(Key, Expr)], out: &mut Sink<'_>) {
    unsigned(fields.len() as u64, out);
    for (key, value) in fields {
        encode_key(key, out);
        encode_expr(value, out);
    }
}

fn encode_arms(arms: &[(Pattern, Expr)], out: &mut Sink<'_>) {
    unsigned(arms.len() as u64, out);
    for (pattern, body) in arms {
        encode_pattern(pattern, out);
        encode_expr(body, out);
    }
}

fn encode_handlers(handlers: &[Handler], out: &mut Sink<'_>) {
    unsigned(handlers.len() as u64, out);
    for handler in handlers {
        out.slot(handler.ability);
        encode_expr(&handler.handler, out);
        match handler.abort {
            None => out.push(0),
            Some(arity) => {
                out.push(1);
                unsigned(arity as u64, out);
            }
        }
    }
}
