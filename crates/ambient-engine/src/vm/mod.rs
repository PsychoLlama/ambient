//! Stack-based virtual machine for executing Ambient bytecode.
//!
//! # Architecture
//!
//! The VM executes compiled functions using four primary data structures:
//!
//! - **Value Stack**: Holds operands for bytecode operations. Push/pop semantics
//!   with type checking at runtime (e.g., `pop_number`, `pop_bool`).
//!
//! - **Call Stack**: Tracks active function calls. Each [`CallFrame`] contains:
//!   - Instruction pointer (IP) into the function's bytecode
//!   - Base pointer (BP) into the value stack for locals
//!   - Captured environment for closures
//!
//! - **Handler Stack**: Manages installed ability handlers. When an ability
//!   operation is performed, the VM searches handlers from most recent to oldest.
//!
//! - **Function Store**: Content-addressed storage mapping `blake3::Hash` to
//!   [`CompiledFunction`]. Functions are identified by their content hash,
//!   enabling deduplication and caching.
//!
//! # Bytecode Execution
//!
//! The main execution loop in [`dispatch`] reads opcodes and dispatches to
//! handlers. Key opcode categories:
//!
//! - **Stack operations**: `Push`, `Pop`, `Dup`, `LoadLocal`, `StoreLocal`
//! - **Arithmetic**: `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Neg`
//! - **Comparison**: `Eq`, `Lt`, `Le`, `Gt`, `Ge`
//! - **Control flow**: `Jump`, `JumpIfTrue`, `JumpIfFalse`, `Call`, `Return`
//! - **Abilities**: `Suspend`, `Resume`, `Handle`, `Unhandle`
//! - **Data structures**: `MakeRecord`, `RecordGet`, `MakeTuple`, `TupleGet`
//!
//! # Error Handling
//!
//! All errors are reported through [`VmError`], which covers:
//! - Stack underflow
//! - Type mismatches (e.g., adding string to number)
//! - Unhandled abilities
//! - Missing functions
//! - Call stack overflow
//!
//! # Module Organization
//!
//! - [`core`]: VM struct, call frames, handler frames, helper methods
//! - [`dispatch`]: Main opcode dispatch loop
//! - [`abilities`]: Ability handling operations (suspend, perform, resume)
//!
//! [`CallFrame`]: core::CallFrame
//! [`CompiledFunction`]: crate::bytecode::CompiledFunction

#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]

mod abilities;
mod core;
mod dispatch;

pub use ambient_ability::{RuntimeError, StackTraceFrame, VmError};
pub use core::Vm;

#[cfg(test)]
mod never_tests;
#[cfg(test)]
mod tests;
