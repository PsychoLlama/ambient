//! Opcode dispatch for the VM execution loop.

use std::sync::Arc;

use ambient_ability::{Value, VmError};

use crate::bytecode::Opcode;

use super::core::Vm;

impl Vm {
    /// Main execution loop. Runs until the frame at depth `base_frames`
    /// returns — 0 for a top-level `call`, the entry depth of a reentrant
    /// [`Vm::invoke`] for a nested region.
    #[allow(clippy::too_many_lines)]
    pub(super) fn run_until(&mut self, base_frames: usize) -> Result<Value, VmError> {
        // How many opcodes run between checks of the host's hard-stop
        // flag: small enough that "the runtime's next opportunity" is
        // effectively immediate, large enough that the atomic load never
        // shows up in the loop's profile.
        const INTERRUPT_CHECK_INTERVAL: u32 = 64;
        let mut interrupt_fuel = INTERRUPT_CHECK_INTERVAL;

        loop {
            interrupt_fuel -= 1;
            if interrupt_fuel == 0 {
                interrupt_fuel = INTERRUPT_CHECK_INTERVAL;
                if let Some(flag) = &self.interrupt
                    && flag.load(std::sync::atomic::Ordering::Relaxed)
                {
                    return Err(VmError::HardStopped);
                }
            }

            let op = self.fetch_opcode()?;

            match op {
                Opcode::PushConst => {
                    let idx = self.read_u16()?;
                    let value = self.get_constant(idx)?;
                    self.stack.push(value);
                }

                Opcode::Pop => {
                    self.pop()?;
                }

                Opcode::Dup => {
                    let value = self.peek()?.clone();
                    self.stack.push(value);
                }

                Opcode::LoadObject => {
                    let idx = self.read_u16()?;
                    let hash = match self.get_constant(idx)? {
                        Value::ObjectRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "object reference",
                                got: other.type_name(),
                                operation: "load_object",
                            });
                        }
                    };
                    let value = self
                        .values
                        .get(&hash)
                        .cloned()
                        .ok_or(VmError::UnknownObject(hash))?;
                    self.stack.push(value);
                }

                Opcode::StoreLocal => {
                    let slot = self.read_u16()?;
                    let value = self.peek()?.clone();
                    self.set_local(slot, value)?;
                }

                Opcode::LoadLocal => {
                    let slot = self.read_u16()?;
                    let value = self.get_local(slot)?;
                    self.stack.push(value);
                }

                // The type checker admits `+` on two numbers or two strings
                // (concatenation); types are erased, so dispatch on the
                // runtime values.
                Opcode::Add => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    match (a, b) {
                        (Value::Number(a), Value::Number(b)) => {
                            self.stack.push(Value::Number(a + b));
                        }
                        (Value::String(a), Value::String(b)) => {
                            let mut result = (*a).clone();
                            result.push_str(&b);
                            self.stack.push(Value::string(result));
                        }
                        (a, _) => {
                            return Err(VmError::TypeError {
                                expected: "two numbers or two strings",
                                got: a.type_name(),
                                operation: "add",
                            });
                        }
                    }
                }
                Opcode::Sub => self.binary_number_op(|a, b| a - b, "sub")?,
                Opcode::Mul => self.binary_number_op(|a, b| a * b, "mul")?,
                Opcode::Div => {
                    let b = self.pop_number("div")?;
                    let a = self.pop_number("div")?;
                    if b == 0.0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.stack.push(Value::Number(a / b));
                }
                Opcode::Mod => {
                    let b = self.pop_number("mod")?;
                    let a = self.pop_number("mod")?;
                    if b == 0.0 {
                        return Err(VmError::DivisionByZero);
                    }
                    self.stack.push(Value::Number(a % b));
                }
                Opcode::Neg => self.unary_number_op(|n| -n, "neg")?,

                // Math functions (unary)
                // Math functions (binary)
                Opcode::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a == b));
                }
                Opcode::Ne => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a != b));
                }
                Opcode::Lt => self.comparison_op(|a, b| a < b, "lt")?,
                Opcode::Le => self.comparison_op(|a, b| a <= b, "le")?,
                Opcode::Gt => self.comparison_op(|a, b| a > b, "gt")?,
                Opcode::Ge => self.comparison_op(|a, b| a >= b, "ge")?,

                Opcode::And => {
                    let b = self.pop_bool("and")?;
                    let a = self.pop_bool("and")?;
                    self.stack.push(Value::Bool(a && b));
                }
                Opcode::Or => {
                    let b = self.pop_bool("or")?;
                    let a = self.pop_bool("or")?;
                    self.stack.push(Value::Bool(a || b));
                }
                Opcode::Not => {
                    let v = self.pop_bool("not")?;
                    self.stack.push(Value::Bool(!v));
                }

                Opcode::Jump => {
                    let offset = self.read_i16()?;
                    self.jump_relative(offset)?;
                }
                Opcode::JumpIf => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool("jump_if")?;
                    if cond {
                        self.jump_relative(offset)?;
                    }
                }
                Opcode::JumpIfNot => {
                    let offset = self.read_i16()?;
                    let cond = self.pop_bool("jump_if_not")?;
                    if !cond {
                        self.jump_relative(offset)?;
                    }
                }

                Opcode::Call => {
                    let func_idx = self.read_u16()?;
                    let arg_count = self.read_u8()?;
                    let func_ref = self.get_constant(func_idx)?;
                    let hash = match func_ref {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "call",
                            });
                        }
                    };
                    self.push_frame(&hash, arg_count)?;
                }

                Opcode::Return => {
                    let result = self.pop()?;

                    // Get info before popping frame
                    let frame = self.frames.pop().ok_or(VmError::StackUnderflow)?;

                    // Pop locals and arguments from stack
                    self.stack.truncate(frame.bp);

                    if self.frames.len() <= base_frames {
                        // Returning from this execution region's entry
                        // function (top-level call, or the callee of a
                        // reentrant invoke). `<` can only happen if a
                        // handler below a stale boundary fired — defensive,
                        // the barrier forbids it.
                        return Ok(result);
                    }

                    // Push result for caller
                    self.stack.push(result);
                }

                Opcode::MakeTuple => {
                    let arity = self.read_u8()?;
                    let mut elements = Vec::with_capacity(arity as usize);
                    for _ in 0..arity {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    self.stack.push(Value::tuple(elements));
                }

                Opcode::TupleGet => {
                    let index = self.read_u8()?;
                    let tuple = self.pop()?;
                    match tuple {
                        Value::Tuple(elements) => {
                            let elem = elements.get(index as usize).ok_or(
                                VmError::TupleIndexOutOfBounds {
                                    index,
                                    length: elements.len(),
                                },
                            )?;
                            self.stack.push(elem.clone());
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "tuple",
                                got: other.type_name(),
                                operation: "tuple_get",
                            });
                        }
                    }
                }

                Opcode::MakeRecord => {
                    let field_count = self.read_u8()?;
                    let mut fields: Vec<(Arc<str>, Value)> =
                        Vec::with_capacity(field_count as usize);

                    // Pop field-value pairs (value first, then field name)
                    for _ in 0..field_count {
                        let value = self.pop()?;
                        let field_name = match self.pop()? {
                            Value::String(s) => Arc::from(s.as_str()),
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "String",
                                    got: other.type_name(),
                                    operation: "make_record",
                                });
                            }
                        };
                        fields.push((field_name, value));
                    }

                    self.stack.push(Value::record(fields));
                }

                Opcode::RecordGet => {
                    let field_idx = self.read_u16()?;
                    let field_name = match self.get_constant(field_idx)? {
                        Value::String(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: other.type_name(),
                                operation: "record_get",
                            });
                        }
                    };

                    let record = self.pop()?;
                    match record {
                        Value::Record(fields) => {
                            let key: Arc<str> = Arc::from(field_name.as_str());
                            let value = fields.get(&key).ok_or_else(|| {
                                VmError::RecordFieldNotFound(field_name.to_string())
                            })?;
                            self.stack.push(value.clone());
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "record",
                                got: other.type_name(),
                                operation: "record_get",
                            });
                        }
                    }
                }

                // ─────────────────────────────────────────────────────────────
                // Abilities (Milestone 2)
                // See vm/abilities.rs for the implementation of these operations.
                // ─────────────────────────────────────────────────────────────
                Opcode::Suspend => {
                    let method_idx = self.read_u16()?;
                    let arg_count = self.read_u8()?;
                    let method_ref = match self.get_constant(method_idx)? {
                        Value::AbilityMethodRef(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "ability method",
                                got: other.type_name(),
                                operation: "suspend",
                            });
                        }
                    };
                    // Reuse the key derived once at load time; fall back to
                    // deriving it only if the cache somehow lacks this
                    // constant (it never should — same source of truth).
                    let method_key = self
                        .current_frame()?
                        .function
                        .method_key(method_idx)
                        .unwrap_or_else(|| method_ref.method_key());
                    self.op_suspend(&method_ref, method_key, arg_count)?;
                }

                Opcode::Perform => {
                    self.op_perform()?;
                }

                Opcode::Unhandle => {
                    self.handlers.pop();
                }

                Opcode::Resume => {
                    self.op_resume()?;
                }

                Opcode::GetAbilityArg => {
                    let arg_index = self.read_u8()? as usize;
                    self.op_get_ability_arg(arg_index)?;
                }

                Opcode::Halt => {
                    return self.pop();
                }

                // ─────────────────────────────────────────────────────────────
                // Closures
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeClosure => {
                    let func_idx = self.read_u16()?;
                    let capture_count = self.read_u8()?;

                    // Get the function hash from the constant pool.
                    let func_hash = match self.get_constant(func_idx)? {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "make_closure",
                            });
                        }
                    };

                    // Pop captured values from the stack (in reverse order).
                    let mut environment = Vec::with_capacity(capture_count as usize);
                    for _ in 0..capture_count {
                        environment.push(self.pop()?);
                    }
                    environment.reverse(); // Restore original capture order

                    // Create and push the closure value.
                    self.stack.push(Value::closure(func_hash, environment));
                }

                Opcode::CallClosure => {
                    let arg_count = self.read_u8()?;

                    // The closure was pushed first, then arguments.
                    // Pop arguments first to get to the closure.
                    let mut args = Vec::with_capacity(arg_count as usize);
                    for _ in 0..arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    // Now pop the callee: a closure, or a bare function
                    // reference (a first-class named function or extern fn
                    // has no environment to capture).
                    let (function_hash, environment) = match self.pop()? {
                        Value::Closure(c) => (c.function_hash, c.environment.clone()),
                        Value::FunctionRef(hash) => (hash, Vec::new()),
                        other => {
                            return Err(VmError::TypeError {
                                expected: "closure",
                                got: other.type_name(),
                                operation: "call_closure",
                            });
                        }
                    };

                    // Push arguments back onto the stack for the call.
                    for arg in args {
                        self.stack.push(arg);
                    }

                    // Call the function with its captured environment.
                    self.push_frame_with_captures(&function_hash, arg_count, environment)?;
                }

                Opcode::LoadCapture => {
                    let capture_slot = self.read_u16()?;

                    // Get the captured value from the current frame's captures.
                    let value = {
                        let frame = self.current_frame()?;
                        frame
                            .captures
                            .get(capture_slot as usize)
                            .cloned()
                            .ok_or(VmError::InvalidLocal(capture_slot))?
                    };
                    self.stack.push(value);
                }

                Opcode::MakeHandler => {
                    let ability_idx = self.read_u16()?;
                    let method_count = self.read_u8()?;
                    let ability_id = match self.get_constant(ability_idx)? {
                        Value::AbilityRef(id) => id,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "ability",
                                got: other.type_name(),
                                operation: "make_handler",
                            });
                        }
                    };
                    let capture_count = self.read_u8()?;

                    // Read method mappings: each arm names its method through
                    // an ability-method constant, keyed by derived MethodKey.
                    let mut methods =
                        std::collections::HashMap::with_capacity(method_count as usize);
                    for _ in 0..method_count {
                        let method_idx = self.read_u16()?;
                        let func_idx = self.read_u16()?;

                        let method_key = match self.get_constant(method_idx)? {
                            Value::AbilityMethodRef(m) => self
                                .current_frame()?
                                .function
                                .method_key(method_idx)
                                .unwrap_or_else(|| m.method_key()),
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "ability method",
                                    got: other.type_name(),
                                    operation: "make_handler",
                                });
                            }
                        };

                        // Get the function hash from the constant pool.
                        let func_hash = match self.get_constant(func_idx)? {
                            Value::FunctionRef(h) => h,
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "function",
                                    got: other.type_name(),
                                    operation: "make_handler",
                                });
                            }
                        };

                        methods.insert(method_key, func_hash);
                    }

                    // Pop captured values from the stack (in reverse order).
                    let mut captures = Vec::with_capacity(capture_count as usize);
                    for _ in 0..capture_count {
                        captures.push(self.pop()?);
                    }
                    captures.reverse(); // Restore original capture order

                    // Create and push the handler value.
                    self.stack.push(Value::Handler(std::sync::Arc::new(
                        crate::value::HandlerValue::with_captures(ability_id, methods, captures),
                    )));
                }

                Opcode::HandleWithValue => {
                    self.op_handle_with_value()?;
                }

                // ─────────────────────────────────────────────────────────────
                // Lists (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeList => {
                    let count = self.read_u16()?;
                    let mut elements = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        elements.push(self.pop()?);
                    }
                    elements.reverse(); // Stack order is reversed
                    self.stack.push(Value::list(elements));
                }

                // ─────────────────────────────────────────────────────────────
                // String operations (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                // ─────────────────────────────────────────────────────────────
                // Type conversion (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                // ─────────────────────────────────────────────────────────────
                // Maps (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                // ─────────────────────────────────────────────────────────────
                // Sets (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeSet => {
                    let count = self.read_u16()? as usize;
                    let mut elements = Vec::with_capacity(count);
                    for _ in 0..count {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    self.stack.push(Value::set(elements));
                }

                // ─────────────────────────────────────────────────────────────
                // Enum operations
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeEnum => {
                    let type_name_idx = self.read_u16()?;
                    let tag = self.read_u16()?;
                    let variant_name_idx = self.read_u16()?;
                    let has_payload = self.read_u8()? != 0;

                    // Get the type name string from constants
                    let type_name = match self.get_constant(type_name_idx)? {
                        Value::String(s) => (*s).clone(),
                        other => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: other.type_name(),
                                operation: "make_enum type_name",
                            });
                        }
                    };

                    // Get the variant name string from constants
                    let variant_name = match self.get_constant(variant_name_idx)? {
                        Value::String(s) => (*s).clone(),
                        other => {
                            return Err(VmError::TypeError {
                                expected: "String",
                                got: other.type_name(),
                                operation: "make_enum variant_name",
                            });
                        }
                    };

                    let payload = if has_payload { Some(self.pop()?) } else { None };

                    let enum_val = Value::enum_variant(&*type_name, tag, &*variant_name, payload);
                    self.stack.push(enum_val);
                }

                Opcode::EnumIs => {
                    let expected_tag = self.read_u16()?;
                    let enum_val = self.peek()?;

                    let result = match enum_val {
                        Value::Enum(e) => e.tag == expected_tag,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "enum",
                                got: other.type_name(),
                                operation: "enum_is",
                            });
                        }
                    };

                    self.stack.push(Value::Bool(result));
                }

                Opcode::EnumPayload => {
                    let enum_val = self.pop()?;

                    match enum_val {
                        Value::Enum(e) => {
                            if let Some(payload) = e.payload.as_deref() {
                                self.stack.push(payload.clone());
                            } else {
                                return Err(VmError::EnumPayloadMissing {
                                    type_name: e.type_name.to_string(),
                                    variant_name: e.variant_name.to_string(),
                                });
                            }
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "enum",
                                got: other.type_name(),
                                operation: "enum_payload",
                            });
                        }
                    }
                } // ─────────────────────────────────────────────────────────────
                  // Protocol serialization
                  // ─────────────────────────────────────────────────────────────
                  // ─────────────────────────────────────────────────────────────
                  // Binary operations
                  // ─────────────────────────────────────────────────────────────
            }
        }
    }
}
