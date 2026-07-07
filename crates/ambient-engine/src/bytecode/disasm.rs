//! Bytecode disassembler for introspection tooling.
//!
//! Produces a human-readable listing of a compiled function's instructions,
//! resolving constant-pool operands inline. The operand table mirrors the
//! VM's dispatch loop — when adding operands to an opcode in
//! `vm/dispatch.rs`, update [`operands`] here to match.

use crate::value::Value;

use super::{CompiledFunction, Opcode};

/// The operand shape that follows an opcode byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operands {
    /// No operands.
    None,
    /// One unsigned byte.
    U8,
    /// One unsigned 16-bit value.
    U16,
    /// One signed 16-bit jump offset.
    I16,
    /// u16 followed by u8 (e.g. `Call`: constant index + arg count).
    U16U8,
    /// u16, u16, u8 (`Suspend`: ability constant index, method, arg count).
    U16U16U8,
    /// u16, u16, u16, u8 (`MakeEnum`).
    U16U16U16U8,
    /// `MakeHandler`: u16 ability constant index, u8 method count,
    /// u8 capture count, then per method (u16 method id, u16 function
    /// constant index).
    Handler,
}

/// Operand shape for an opcode. Mirrors `vm/dispatch.rs`.
fn operands(op: Opcode) -> Operands {
    use Opcode as O;
    match op {
        O::PushConst
        | O::LoadObject
        | O::StoreLocal
        | O::LoadLocal
        | O::RecordGet
        | O::LoadCapture
        | O::MakeList
        | O::MakeSet
        | O::EnumIs => Operands::U16,

        O::Jump | O::JumpIf | O::JumpIfNot => Operands::I16,

        O::MakeTuple | O::TupleGet | O::MakeRecord | O::GetAbilityArg | O::CallClosure => {
            Operands::U8
        }

        O::Call | O::MakeClosure => Operands::U16U8,
        O::Suspend => Operands::U16U16U8,
        O::MakeEnum => Operands::U16U16U16U8,
        O::MakeHandler => Operands::Handler,

        _ => Operands::None,
    }
}

/// Render a constant-pool value compactly for listings.
fn format_constant(value: &Value) -> String {
    match value {
        Value::FunctionRef(hash) => {
            let hex = hash.to_hex();
            format!("fn {}", &hex.as_str()[..12])
        }
        Value::ObjectRef(hash) => {
            let hex = hash.to_hex();
            format!("const {}", &hex.as_str()[..12])
        }
        Value::AbilityRef(id) => format!("ability {}", id.short_hex()),
        Value::String(s) => {
            let s = s.as_str();
            if s.chars().count() > 40 {
                let truncated: String = s.chars().take(40).collect();
                format!("{truncated:?}…")
            } else {
                format!("{s:?}")
            }
        }
        other => crate::format::format_value(other),
    }
}

/// Byte cursor over a function's bytecode.
struct Cursor<'a> {
    code: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = self.code.get(self.pos).copied();
        self.pos += 1;
        v
    }

    fn u16(&mut self) -> Option<u16> {
        let lo = self.code.get(self.pos).copied()?;
        let hi = self.code.get(self.pos + 1).copied()?;
        self.pos += 2;
        Some(u16::from_le_bytes([lo, hi]))
    }
}

/// Disassemble a function's bytecode into a readable listing.
///
/// Unknown opcodes and truncated operands are rendered as `??` lines rather
/// than failing: the disassembler is a diagnostic tool and should show as
/// much as it can.
#[must_use]
pub fn disassemble(func: &CompiledFunction) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let mut cur = Cursor {
        code: &func.bytecode,
        pos: 0,
    };

    while cur.pos < cur.code.len() {
        let offset = cur.pos;
        let byte = cur.code[cur.pos];
        cur.pos += 1;

        match Opcode::from_byte(byte) {
            Some(op) => {
                let detail = instruction_detail(func, op, offset, &mut cur);
                let _ = writeln!(out, "{offset:04}  {op:?}{detail}");
            }
            None => {
                let _ = writeln!(out, "{offset:04}  ?? 0x{byte:02x}");
            }
        }
    }

    out
}

/// Render the operands of one instruction, advancing the cursor.
fn instruction_detail(
    func: &CompiledFunction,
    op: Opcode,
    offset: usize,
    cur: &mut Cursor<'_>,
) -> String {
    use std::fmt::Write;

    const TRUNCATED: &str = " <truncated>";

    let const_at = |idx: u16| -> String {
        func.constants.get(idx as usize).map_or_else(
            || format!("<bad const #{idx}>"),
            |v| format!("#{idx} = {}", format_constant(v)),
        )
    };

    match operands(op) {
        Operands::None => String::new(),
        Operands::U8 => cur
            .u8()
            .map_or_else(|| TRUNCATED.to_string(), |v| format!(" {v}")),
        Operands::U16 => match cur.u16() {
            Some(v) if op == Opcode::PushConst || op == Opcode::LoadObject => {
                format!(" {}", const_at(v))
            }
            Some(v) => format!(" {v}"),
            None => TRUNCATED.to_string(),
        },
        Operands::I16 => match cur.u16() {
            #[allow(clippy::cast_possible_wrap)]
            Some(v) => {
                let rel = v as i16;
                let target = offset as i64 + 3 + i64::from(rel);
                format!(" {rel:+} (-> {target:04})")
            }
            None => TRUNCATED.to_string(),
        },
        Operands::U16U8 => match (cur.u16(), cur.u8()) {
            (Some(a), Some(b)) if op == Opcode::Call || op == Opcode::MakeClosure => {
                format!(" {}, n={b}", const_at(a))
            }
            (Some(a), Some(b)) => format!(" {a}, {b}"),
            _ => TRUNCATED.to_string(),
        },
        Operands::U16U16U8 => match (cur.u16(), cur.u16(), cur.u8()) {
            (Some(a), Some(b), Some(c)) => {
                format!(" ability={}, method={b}, n={c}", const_at(a))
            }
            _ => TRUNCATED.to_string(),
        },
        Operands::U16U16U16U8 => match (cur.u16(), cur.u16(), cur.u16(), cur.u8()) {
            (Some(a), Some(b), Some(c), Some(d)) => {
                format!(
                    " type={}, tag={b}, variant={}, payload={d}",
                    const_at(a),
                    const_at(c)
                )
            }
            _ => TRUNCATED.to_string(),
        },
        Operands::Handler => match (cur.u16(), cur.u8(), cur.u8()) {
            (Some(ability), Some(methods), Some(captures)) => {
                let mut s = format!(
                    " ability={}, methods={methods}, captures={captures}",
                    const_at(ability)
                );
                for _ in 0..methods {
                    if let (Some(id), Some(func_idx)) = (cur.u16(), cur.u16()) {
                        let _ = write!(s, "\n{:>10}method {id} -> {}", "", const_at(func_idx));
                    } else {
                        s.push_str(TRUNCATED);
                        break;
                    }
                }
                s
            }
            _ => TRUNCATED.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::BytecodeBuilder;

    #[test]
    fn disassembles_simple_function() {
        let mut builder = BytecodeBuilder::new();
        builder.emit_const(Value::Number(42.0));
        builder.emit(Opcode::Return);
        let func = builder.build(0, 0);

        let listing = disassemble(&func);
        assert!(listing.contains("PushConst"), "listing: {listing}");
        assert!(listing.contains("42"), "listing: {listing}");
        assert!(listing.contains("Return"), "listing: {listing}");
    }

    #[test]
    fn disassembles_calls_with_function_refs() {
        let dep = blake3::hash(b"callee");
        let mut builder = BytecodeBuilder::new();
        builder.emit_call(dep, 2);
        builder.emit(Opcode::Return);
        let func = builder.build_with_dependencies(0, 0, vec![dep]);

        let listing = disassemble(&func);
        assert!(listing.contains("Call"), "listing: {listing}");
        assert!(
            listing.contains(&dep.to_hex().as_str()[..12]),
            "listing should show the callee hash prefix: {listing}"
        );
    }

    #[test]
    fn unknown_bytes_do_not_panic() {
        let func = CompiledFunction {
            hash: blake3::hash(b"x"),
            bytecode: vec![0x0E, 0xFF, 0x00, 0x50],
            constants: vec![],
            local_count: 0,
            param_count: 0,
            dependencies: vec![],
            debug_info: None,
        };
        let listing = disassemble(&func);
        assert!(listing.contains("??"), "listing: {listing}");
    }
}
