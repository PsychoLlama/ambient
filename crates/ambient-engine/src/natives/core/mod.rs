//! The engine's native implementations for `core_lib`'s `extern fn`
//! declarations, one submodule per core module.
//!
//! Every function here is a pure value transformation ported from the old
//! intrinsic opcode handlers; the `.ab` sources own the signatures and doc
//! comments, this side owns the behavior and the stable UUIDs.
//!
//! # UUID discipline
//!
//! Core native UUIDs live in the reserved block
//! `FFFFFFFF-FFFF-FFFF-FFFE-XXXXXXXXXXXX` ([`uuid`]), assigned in
//! per-module ranges (`0x01__` number, `0x02__` string, `0x03__` list,
//! `0x04__` map, `0x05__` set, `0x06__` binary, `0x07__` convert,
//! `0x08__` reflect, `0x09__` protocol). An assigned id is **permanent**:
//! it names a behavior, compiled code links to it by hash, and remote
//! hosts bind it by id — never reuse or renumber one. Removing a function
//! retires its id; changing semantics mints a new id.

use std::sync::Arc;

use crate::{Value, VmError};

use super::NativeRegistry;

mod binary;
mod collections;
mod convert;
mod number;
mod protocol;
mod reflect;
mod string;

/// Assemble the full core registry.
pub(super) fn registry() -> NativeRegistry {
    let mut reg = NativeRegistry::new();
    number::register(&mut reg);
    string::register(&mut reg);
    collections::register(&mut reg);
    binary::register(&mut reg);
    convert::register(&mut reg);
    reflect::register(&mut reg);
    protocol::register(&mut reg);
    reg
}

/// A core native UUID: the reserved `…FFFE` block plus a permanent id.
pub(super) const fn uuid(id: u64) -> uuid::Uuid {
    uuid::Uuid::from_u128(0xFFFF_FFFF_FFFF_FFFF_FFFE_0000_0000_0000 | id as u128)
}

/// Register one binding under a core module path.
pub(super) fn bind(
    reg: &mut NativeRegistry,
    module: &crate::module_path::ModulePath,
    name: &'static str,
    id: u64,
    arity: u8,
    func: fn(Vec<Value>) -> Result<Value, VmError>,
) {
    reg.register(module, name, uuid(id), arity, Arc::new(func));
}

/// A core module path from static segments.
pub(super) fn module(segments: &[&str]) -> crate::module_path::ModulePath {
    crate::module_path::ModulePath::from_str_segments(segments)
        .unwrap_or_else(|| unreachable!("core module paths are static and valid"))
}

// ─────────────────────────────────────────────────────────────────────────
// Argument extraction
//
// Natives receive their arguments in declaration order. These helpers pull
// one argument out by index with the same runtime type errors the old
// opcode handlers raised.
// ─────────────────────────────────────────────────────────────────────────

pub(super) fn arg(args: &mut [Value], index: usize) -> Value {
    // Arity is checked before dispatch (`Vm::call_native`), so a missing
    // argument is unreachable; Unit keeps this total without panicking.
    if index < args.len() {
        std::mem::replace(&mut args[index], Value::Unit)
    } else {
        Value::Unit
    }
}

pub(super) fn type_error(expected: &'static str, got: &Value, operation: &'static str) -> VmError {
    VmError::TypeError {
        expected,
        got: got.type_name(),
        operation,
    }
}

pub(super) fn number(args: &mut [Value], index: usize, op: &'static str) -> Result<f64, VmError> {
    match arg(args, index) {
        Value::Number(n) => Ok(n),
        other => Err(type_error("Number", &other, op)),
    }
}

pub(super) fn string(
    args: &mut [Value],
    index: usize,
    op: &'static str,
) -> Result<Arc<String>, VmError> {
    match arg(args, index) {
        Value::String(s) => Ok(s),
        other => Err(type_error("String", &other, op)),
    }
}

pub(super) fn list(
    args: &mut [Value],
    index: usize,
    op: &'static str,
) -> Result<Arc<Vec<Value>>, VmError> {
    match arg(args, index) {
        Value::List(elements) => Ok(elements),
        other => Err(type_error("list", &other, op)),
    }
}

pub(super) fn binary_arg(
    args: &mut [Value],
    index: usize,
    op: &'static str,
) -> Result<Arc<Vec<u8>>, VmError> {
    match arg(args, index) {
        Value::Binary(b) => Ok(b),
        other => Err(type_error("Binary", &other, op)),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Index / bound conversion (shared semantics with the VM's former opcode
// handlers — see the originals' comments for the full rationale)
// ─────────────────────────────────────────────────────────────────────────

/// Convert a numeric index into a `usize` for element access (`get`).
/// `None` for anything that cannot address an element: negatives,
/// fractional numbers, NaN/infinity.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub(super) fn usize_index(index: f64) -> Option<usize> {
    if index.is_finite() && index >= 0.0 && index.fract() == 0.0 {
        Some(index as usize)
    } else {
        None
    }
}

/// Clamp a numeric slice bound to a valid `[0, len]` offset: slicing is
/// lenient where element access is strict.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
pub(super) fn slice_bound(index: f64, len: usize) -> usize {
    if index.is_nan() || index <= 0.0 {
        0
    } else if index >= len as f64 {
        len
    } else {
        index as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Call a core native by its assigned id.
    fn call(id: u64, args: Vec<Value>) -> Result<Value, VmError> {
        let native = crate::natives::core_natives()
            .impl_for(&uuid(id))
            .unwrap_or_else(|| panic!("core native 0x{id:04X} is registered"));
        native(args)
    }

    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    fn s(text: &str) -> Value {
        Value::string(text)
    }

    fn nums(values: &[f64]) -> Value {
        Value::list(values.iter().map(|&n| num(n)).collect())
    }

    // Behavioral coverage ported from the former intrinsic-opcode VM tests:
    // the natives must preserve the opcode handlers' exact semantics.

    #[test]
    fn list_get_bounds() {
        let list = || nums(&[10.0, 20.0, 30.0]);
        assert_eq!(
            call(0x0302, vec![list(), num(1.0)]),
            Ok(Value::some(num(20.0)))
        );
        assert_eq!(call(0x0302, vec![list(), num(3.0)]), Ok(Value::none()));
        // Negative and fractional indexes address nothing.
        assert_eq!(call(0x0302, vec![list(), num(-1.0)]), Ok(Value::none()));
        assert_eq!(call(0x0302, vec![list(), num(0.5)]), Ok(Value::none()));
    }

    #[test]
    fn list_shape_ops() {
        let list = || nums(&[1.0, 2.0, 3.0]);
        assert_eq!(call(0x0301, vec![list()]), Ok(num(3.0)));
        assert_eq!(call(0x0303, vec![list()]), Ok(Value::some(num(1.0))));
        assert_eq!(call(0x0303, vec![nums(&[])]), Ok(Value::none()));
        assert_eq!(call(0x0304, vec![list()]), Ok(nums(&[2.0, 3.0])));
        assert_eq!(call(0x0308, vec![list()]), Ok(Value::some(num(3.0))));
        assert_eq!(
            call(0x0305, vec![nums(&[1.0]), nums(&[2.0])]),
            Ok(nums(&[1.0, 2.0]))
        );
        assert_eq!(
            call(0x0306, vec![nums(&[1.0]), num(2.0)]),
            Ok(nums(&[1.0, 2.0]))
        );
        assert_eq!(call(0x0307, vec![nums(&[])]), Ok(Value::Bool(true)));
        assert_eq!(call(0x0309, vec![list()]), Ok(nums(&[3.0, 2.0, 1.0])));
        assert_eq!(
            call(0x030A, vec![nums(&[3.0, 1.0, 2.0])]),
            Ok(nums(&[1.0, 2.0, 3.0]))
        );
        // Slice bounds clamp; negative starts clamp to zero.
        assert_eq!(
            call(0x030B, vec![list(), num(-5.0), num(2.0)]),
            Ok(nums(&[1.0, 2.0]))
        );
    }

    #[test]
    fn string_ops() {
        assert_eq!(call(0x0201, vec![s("hello")]), Ok(num(5.0)));
        assert_eq!(call(0x0202, vec![s("foo"), s("bar")]), Ok(s("foobar")));
        assert_eq!(
            call(0x0203, vec![s("haystack"), s("st")]),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            call(0x0203, vec![s("haystack"), s("xyz")]),
            Ok(Value::Bool(false))
        );
        assert_eq!(
            call(0x0204, vec![s("a,b"), s(",")]),
            Ok(Value::list(vec![s("a"), s("b")]))
        );
        assert_eq!(
            call(0x0205, vec![Value::list(vec![s("a"), s("b")]), s("-")]),
            Ok(s("a-b"))
        );
        assert_eq!(call(0x0206, vec![s("  hi  ")]), Ok(s("hi")));
        assert_eq!(
            call(0x0207, vec![s("hello"), num(1.0), num(3.0)]),
            Ok(s("el"))
        );
        assert_eq!(
            call(0x020E, vec![s("hello"), s("ll")]),
            Ok(Value::some(num(2.0)))
        );
        assert_eq!(call(0x0210, vec![s("abc")]), Ok(s("cba")));
    }

    #[test]
    fn conversions() {
        assert_eq!(call(0x0701, vec![num(42.0)]), Ok(s("42")));
        assert_eq!(call(0x0701, vec![Value::Bool(true)]), Ok(s("true")));
        assert_eq!(call(0x0702, vec![s(" 3.5 ")]), Ok(Value::some(num(3.5))));
        assert_eq!(call(0x0702, vec![s("nope")]), Ok(Value::none()));
        assert_eq!(
            call(0x0703, vec![s("yes")]),
            Ok(Value::some(Value::Bool(true)))
        );
        assert_eq!(call(0x0703, vec![s("maybe")]), Ok(Value::none()));
    }

    #[test]
    fn set_ops() {
        let set_ab = call(0x0502, vec![call(0x0501, vec![]).unwrap(), num(1.0)]).unwrap();
        let set_ab = call(0x0502, vec![set_ab, num(2.0)]).unwrap();
        // Duplicate insert is a no-op.
        let set_ab = call(0x0502, vec![set_ab, num(2.0)]).unwrap();
        assert_eq!(call(0x0505, vec![set_ab.clone()]), Ok(num(2.0)));
        assert_eq!(
            call(0x0504, vec![set_ab.clone(), num(2.0)]),
            Ok(Value::Bool(true))
        );
        let removed = call(0x0503, vec![set_ab.clone(), num(2.0)]).unwrap();
        assert_eq!(
            call(0x0504, vec![removed, num(2.0)]),
            Ok(Value::Bool(false))
        );

        let set_bc = call(0x0502, vec![call(0x0501, vec![]).unwrap(), num(2.0)]).unwrap();
        let set_bc = call(0x0502, vec![set_bc, num(3.0)]).unwrap();
        let union = call(0x0506, vec![set_ab.clone(), set_bc.clone()]).unwrap();
        assert_eq!(call(0x0505, vec![union]), Ok(num(3.0)));
        let inter = call(0x0507, vec![set_ab.clone(), set_bc.clone()]).unwrap();
        assert_eq!(call(0x0505, vec![inter]), Ok(num(1.0)));
        let diff = call(0x0508, vec![set_ab, set_bc]).unwrap();
        assert_eq!(call(0x0505, vec![diff]), Ok(num(1.0)));
    }

    #[test]
    fn binary_ops() {
        let bin = call(0x0601, vec![nums(&[1.0, 2.0, 3.0])]).unwrap();
        assert_eq!(call(0x0603, vec![bin.clone()]), Ok(num(3.0)));
        assert_eq!(
            call(0x0604, vec![bin.clone(), num(1.0)]),
            Ok(Value::some(num(2.0)))
        );
        // Negative indexes address nothing (never alias element 0).
        assert_eq!(
            call(0x0604, vec![bin.clone(), num(-1.0)]),
            Ok(Value::none())
        );
        assert_eq!(call(0x0602, vec![bin]), Ok(nums(&[1.0, 2.0, 3.0])));
    }

    #[test]
    fn reflect_tag() {
        assert_eq!(call(0x0801, vec![Value::some(num(1.0))]), Ok(num(1.0)));
        assert_eq!(call(0x0801, vec![Value::none()]), Ok(num(0.0)));
    }

    #[test]
    fn math() {
        assert_eq!(call(0x0101, vec![num(16.0)]), Ok(num(4.0)));
        assert_eq!(call(0x010C, vec![num(2.0), num(10.0)]), Ok(num(1024.0)));
        assert_eq!(call(0x010D, vec![num(3.0), num(7.0)]), Ok(num(3.0)));
    }

    /// The uuid table is permanent: every (module, name) → id assignment is
    /// pinned here byte-for-byte, so an accidental renumbering — which would
    /// orphan every compiled caller — fails this test instead of shipping.
    #[test]
    fn native_uuid_assignments_are_pinned() {
        let reg = crate::natives::core_natives();
        let expect = |segments: &[&str], name: &str, id: u64, arity: u8| {
            let key = reg
                .key_for(&module(segments), name)
                .unwrap_or_else(|| panic!("{segments:?}::{name} is bound"));
            assert_eq!(key.uuid, uuid(id), "{segments:?}::{name} uuid drifted");
            assert_eq!(key.arity, arity, "{segments:?}::{name} arity drifted");
        };

        let number = &["core", "primitives", "number"];
        for (i, name) in [
            "sqrt", "abs", "floor", "ceil", "round", "trunc", "sin", "cos", "tan", "ln", "exp",
        ]
        .iter()
        .enumerate()
        {
            expect(number, name, 0x0101 + i as u64, 1);
        }
        expect(number, "pow", 0x010C, 2);
        expect(number, "min", 0x010D, 2);
        expect(number, "max", 0x010E, 2);
        expect(number, "atan2", 0x0112, 2);
        expect(number, "log2", 0x0114, 1);

        let string = &["core", "primitives", "string"];
        expect(string, "length", 0x0201, 1);
        expect(string, "concat", 0x0202, 2);
        expect(string, "join", 0x0205, 2);
        expect(string, "reverse", 0x0210, 1);

        let list = &["core", "collections", "list"];
        expect(list, "length", 0x0301, 1);
        expect(list, "get", 0x0302, 2);
        expect(list, "slice", 0x030B, 3);

        expect(&["core", "collections", "map"], "empty", 0x0401, 0);
        expect(&["core", "collections", "map"], "values", 0x0408, 1);
        expect(&["core", "collections", "set"], "empty", 0x0501, 0);
        expect(&["core", "collections", "set"], "to_list", 0x0509, 1);
        expect(&["core", "primitives", "binary"], "from", 0x0601, 1);
        expect(&["core", "primitives", "binary"], "concat", 0x0606, 2);
        expect(&["core", "convert"], "to_string", 0x0701, 1);
        expect(&["core", "convert"], "parse_bool", 0x0703, 1);
        expect(&["core", "reflect"], "tag", 0x0801, 1);
        expect(&["core", "reflect"], "payload", 0x0802, 1);
        expect(&["core", "protocol"], "serialize_value", 0x0901, 1);
        expect(&["core", "protocol"], "binary_to_hex", 0x0907, 1);
    }
}
