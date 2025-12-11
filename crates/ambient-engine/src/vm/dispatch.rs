//! Opcode dispatch for the VM execution loop.

use std::sync::Arc;

use crate::bytecode::Opcode;
use crate::value::{CapturedFrame, Value};

use super::core::{CallFrame, HandlerFrame, HandlerKind, ReturnAction, Vm};
use super::error::VmError;

impl Vm {
    /// Main execution loop.
    #[allow(clippy::too_many_lines)]
    pub(super) fn run(&mut self) -> Result<Value, VmError> {
        loop {
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

                Opcode::Add => self.binary_number_op(|a, b| a + b, "add")?,
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
                Opcode::Sqrt => self.unary_number_op(f64::sqrt, "sqrt")?,
                Opcode::Abs => self.unary_number_op(f64::abs, "abs")?,
                Opcode::Floor => self.unary_number_op(f64::floor, "floor")?,
                Opcode::Ceil => self.unary_number_op(f64::ceil, "ceil")?,
                Opcode::Round => self.unary_number_op(f64::round, "round")?,
                Opcode::Trunc => self.unary_number_op(f64::trunc, "trunc")?,
                Opcode::Sin => self.unary_number_op(f64::sin, "sin")?,
                Opcode::Cos => self.unary_number_op(f64::cos, "cos")?,
                Opcode::Tan => self.unary_number_op(f64::tan, "tan")?,
                Opcode::Ln => self.unary_number_op(f64::ln, "ln")?,
                Opcode::Exp => self.unary_number_op(f64::exp, "exp")?,

                // Math functions (binary)
                Opcode::Pow => self.binary_number_op(f64::powf, "pow")?,
                Opcode::Min => self.binary_number_op(f64::min, "min")?,
                Opcode::Max => self.binary_number_op(f64::max, "max")?,
                Opcode::Asin => self.unary_number_op(f64::asin, "asin")?,
                Opcode::Acos => self.unary_number_op(f64::acos, "acos")?,
                Opcode::Atan => self.unary_number_op(f64::atan, "atan")?,
                Opcode::Atan2 => self.binary_number_op(f64::atan2, "atan2")?,
                Opcode::Log10 => self.unary_number_op(f64::log10, "log10")?,
                Opcode::Log2 => self.unary_number_op(f64::log2, "log2")?,

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
                            })
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

                    // Apply return action to transform the result
                    let final_result = match frame.return_action {
                        ReturnAction::None | ReturnAction::PassThrough => result,
                        ReturnAction::WrapSome => Value::some(result),
                        ReturnAction::WrapOk => Value::ok(result),
                        ReturnAction::WrapErr => Value::err(result),
                    };

                    if self.frames.is_empty() {
                        // Returning from top-level function
                        return Ok(final_result);
                    }

                    // Push result for caller
                    self.stack.push(final_result);
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
                            })
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
                                    expected: "string",
                                    got: other.type_name(),
                                    operation: "make_record",
                                })
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
                                expected: "string",
                                got: other.type_name(),
                                operation: "record_get",
                            })
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
                            })
                        }
                    }
                }

                // ─────────────────────────────────────────────────────────────
                // Abilities (Milestone 2)
                // ─────────────────────────────────────────────────────────────
                Opcode::Suspend => {
                    // Create a suspended ability value from arguments on the stack
                    let ability_id = self.read_u16()?;
                    let method_id = self.read_u16()?;
                    let arg_count = self.read_u8()?;

                    // Pop arguments (in reverse order)
                    let mut args = Vec::with_capacity(arg_count as usize);
                    for _ in 0..arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    // Push the suspended ability value
                    self.stack
                        .push(Value::suspended_ability(ability_id, method_id, args));
                }

                Opcode::Perform => {
                    // Pop the suspended ability and perform it
                    let ability = match self.pop()? {
                        Value::SuspendedAbility(a) => a,
                        other => {
                            return Err(VmError::ExpectedSuspendedAbility {
                                got: other.type_name(),
                            })
                        }
                    };

                    // First, check for a host handler
                    if let Some(handler) = self
                        .host_handlers
                        .get(&(ability.ability_id, ability.method_id))
                    {
                        // Call the host handler synchronously
                        let result = handler(&ability)?;
                        self.stack.push(result);
                        continue;
                    }

                    // Look for a bytecode handler on the handler stack
                    let handler_idx = self
                        .handlers
                        .iter()
                        .rposition(|h| h.ability_id == ability.ability_id);

                    if let Some(idx) = handler_idx {
                        // Found a handler - capture continuation and jump to handler
                        let handler = self.handlers[idx].clone();

                        // Determine the handler function to call based on handler kind
                        let handler_func = match &handler.handler {
                            HandlerKind::Inline { handler_func } => *handler_func,
                            HandlerKind::Value { handler_value } => {
                                // Look up the method function from the handler value
                                match handler_value.get_method(ability.method_id) {
                                    Some(func) => func,
                                    None => {
                                        return Err(VmError::UnhandledAbility {
                                            ability_id: ability.ability_id,
                                            method_id: ability.method_id,
                                        });
                                    }
                                }
                            }
                        };

                        // Capture the continuation: stack and frames from handler point to current
                        let captured_stack = self.stack.split_off(handler.stack_height);
                        let captured_frames: Vec<CapturedFrame> = self.frames
                            [handler.call_frame_idx..]
                            .iter()
                            .map(|f| CapturedFrame {
                                function_hash: f.function.hash,
                                ip: f.ip,
                                bp: f.bp,
                            })
                            .collect();

                        // Truncate frames to handler point
                        self.frames.truncate(handler.call_frame_idx);

                        // Remove the handler (and any handlers installed after it)
                        self.handlers.truncate(idx);

                        // Create continuation value
                        let continuation = Value::continuation(captured_stack, captured_frames);

                        // Push the continuation and the suspended ability as arguments
                        // to the handler function
                        self.stack.push(continuation);
                        self.stack.push(Value::SuspendedAbility(ability));

                        // Call the handler function
                        self.push_frame(&handler_func, 2)?;
                    } else {
                        // No handler found
                        return Err(VmError::UnhandledAbility {
                            ability_id: ability.ability_id,
                            method_id: ability.method_id,
                        });
                    }
                }

                Opcode::Handle => {
                    // Install an ability handler (inline)
                    let ability_id = self.read_u16()?;
                    let handler_idx = self.read_u16()?;
                    let _completion_offset = self.read_i16()?; // Reserved for future optimization

                    let handler_func = match self.get_constant(handler_idx)? {
                        Value::FunctionRef(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "function",
                                got: other.type_name(),
                                operation: "handle",
                            })
                        }
                    };

                    self.handlers.push(HandlerFrame {
                        ability_id,
                        handler: HandlerKind::Inline { handler_func },
                        call_frame_idx: self.frames.len() - 1,
                        stack_height: self.stack.len(),
                    });
                }

                Opcode::Unhandle => {
                    // Remove the most recent handler
                    self.handlers.pop();
                }

                Opcode::Resume => {
                    // Resume a continuation with a value
                    let value = self.pop()?;
                    let continuation = match self.pop()? {
                        Value::Continuation(c) => c,
                        other => {
                            return Err(VmError::ExpectedContinuation {
                                got: other.type_name(),
                            })
                        }
                    };

                    // Single-shot enforcement
                    if !continuation.mark_resumed() {
                        return Err(VmError::ContinuationAlreadyResumed);
                    }

                    // Restore the captured stack
                    self.stack.extend(continuation.stack.iter().cloned());

                    // Restore the captured frames
                    for captured in &continuation.frames {
                        let function = self
                            .functions
                            .get(&captured.function_hash)
                            .ok_or(VmError::UnknownFunction(captured.function_hash))?
                            .clone();

                        self.frames.push(CallFrame {
                            function,
                            ip: captured.ip,
                            bp: captured.bp,
                            captures: Vec::new(), // Continuations don't preserve closure captures
                            return_action: ReturnAction::None, // Restored frames have no special action
                        });
                    }

                    // Push the resume value as the result of the Perform
                    self.stack.push(value);
                }

                Opcode::GetAbilityArg => {
                    let arg_index = self.read_u8()? as usize;
                    let ability = match self.pop()? {
                        Value::SuspendedAbility(a) => a,
                        other => {
                            return Err(VmError::ExpectedSuspendedAbility {
                                got: other.type_name(),
                            })
                        }
                    };

                    if arg_index >= ability.args.len() {
                        return Err(VmError::AbilityArgOutOfBounds {
                            index: arg_index,
                            length: ability.args.len(),
                        });
                    }

                    self.stack.push(ability.args[arg_index].clone());
                }

                Opcode::Halt => {
                    return self.pop();
                }

                // ─────────────────────────────────────────────────────────────
                // Concurrency (Milestone 9)
                // ─────────────────────────────────────────────────────────────
                Opcode::AsyncAll => {
                    let count = self.read_u8()?;

                    // Pop all suspended abilities (in reverse order)
                    let mut abilities = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let ability = match self.pop()? {
                            Value::SuspendedAbility(a) => a,
                            other => {
                                return Err(VmError::ExpectedSuspendedAbility {
                                    got: other.type_name(),
                                })
                            }
                        };
                        abilities.push(ability);
                    }
                    abilities.reverse(); // Restore original order

                    // Perform all abilities and collect results
                    let results = self.perform_all_abilities(&abilities)?;

                    // Push tuple of results
                    self.stack.push(Value::tuple(results));
                }

                Opcode::AsyncRace => {
                    let count = self.read_u8()?;

                    // Pop all suspended abilities (in reverse order)
                    let mut abilities = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let ability = match self.pop()? {
                            Value::SuspendedAbility(a) => a,
                            other => {
                                return Err(VmError::ExpectedSuspendedAbility {
                                    got: other.type_name(),
                                })
                            }
                        };
                        abilities.push(ability);
                    }
                    abilities.reverse(); // Restore original order

                    // Race: perform abilities concurrently, return first result
                    let result = self.perform_race_abilities(&abilities)?;

                    // Push the winning result
                    self.stack.push(result);
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
                            })
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

                    // Now pop the closure.
                    let closure = match self.pop()? {
                        Value::Closure(c) => c,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "closure",
                                got: other.type_name(),
                                operation: "call_closure",
                            })
                        }
                    };

                    // Push arguments back onto the stack for the call.
                    for arg in args {
                        self.stack.push(arg);
                    }

                    // Call the closure's function with its captured environment.
                    self.push_frame_with_captures(
                        &closure.function_hash,
                        arg_count,
                        closure.environment.clone(),
                    )?;
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
                    let ability_id = self.read_u16()?;
                    let method_count = self.read_u8()?;
                    let capture_count = self.read_u8()?;

                    // Read method mappings.
                    let mut methods =
                        std::collections::HashMap::with_capacity(method_count as usize);
                    for _ in 0..method_count {
                        let method_id = self.read_u16()?;
                        let func_idx = self.read_u16()?;

                        // Get the function hash from the constant pool.
                        let func_hash = match self.get_constant(func_idx)? {
                            Value::FunctionRef(h) => h,
                            other => {
                                return Err(VmError::TypeError {
                                    expected: "function",
                                    got: other.type_name(),
                                    operation: "make_handler",
                                })
                            }
                        };

                        methods.insert(method_id, func_hash);
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
                    // Install a handler from a HandlerValue on the stack
                    let _completion_offset = self.read_i16()?; // Reserved for future optimization

                    // Pop the handler value from the stack
                    let handler_value = match self.pop()? {
                        Value::Handler(h) => h,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "handler",
                                got: other.type_name(),
                                operation: "handle_with_value",
                            })
                        }
                    };

                    self.handlers.push(HandlerFrame {
                        ability_id: handler_value.ability_id,
                        handler: HandlerKind::Value { handler_value },
                        call_frame_idx: self.frames.len() - 1,
                        stack_height: self.stack.len(),
                    });
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

                Opcode::ListGet => {
                    let index = self.pop_number("list_get")? as usize;
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_get",
                            })
                        }
                    };
                    let result = list.get(index).cloned().unwrap_or(Value::Unit);
                    self.stack.push(result);
                }

                Opcode::ListLength => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_length",
                            })
                        }
                    };
                    #[allow(clippy::cast_precision_loss)]
                    self.stack.push(Value::Number(list.len() as f64));
                }

                Opcode::ListConcat => {
                    let list2 = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_concat",
                            })
                        }
                    };
                    let list1 = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_concat",
                            })
                        }
                    };
                    let mut result = (*list1).clone();
                    result.extend((*list2).iter().cloned());
                    self.stack.push(Value::list(result));
                }

                Opcode::ListAppend => {
                    let value = self.pop()?;
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_append",
                            })
                        }
                    };
                    let mut result = (*list).clone();
                    result.push(value);
                    self.stack.push(Value::list(result));
                }

                Opcode::ListHead => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_head",
                            })
                        }
                    };
                    let result = list.first().cloned().unwrap_or(Value::Unit);
                    self.stack.push(result);
                }

                Opcode::ListTail => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_tail",
                            })
                        }
                    };
                    let result = if list.len() <= 1 {
                        Vec::new()
                    } else {
                        list[1..].to_vec()
                    };
                    self.stack.push(Value::list(result));
                }

                Opcode::ListReverse => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_reverse",
                            })
                        }
                    };
                    let mut result: Vec<Value> = (*list).clone();
                    result.reverse();
                    self.stack.push(Value::list(result));
                }

                Opcode::ListSort => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_sort",
                            })
                        }
                    };
                    let mut result: Vec<Value> = (*list).clone();
                    result.sort_by(|a, b| match (a, b) {
                        (Value::Number(na), Value::Number(nb)) => {
                            na.partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal)
                        }
                        (Value::String(sa), Value::String(sb)) => sa.cmp(sb),
                        _ => std::cmp::Ordering::Equal,
                    });
                    self.stack.push(Value::list(result));
                }

                Opcode::ListSlice => {
                    let end = self.pop_number("list_slice")? as usize;
                    let start = self.pop_number("list_slice")? as usize;
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_slice",
                            })
                        }
                    };
                    let len = list.len();
                    let start = start.min(len);
                    let end = end.min(len);
                    let result = if start >= end {
                        Vec::new()
                    } else {
                        list[start..end].to_vec()
                    };
                    self.stack.push(Value::list(result));
                }

                Opcode::ListIsEmpty => {
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "list_is_empty",
                            })
                        }
                    };
                    self.stack.push(Value::Bool(list.is_empty()));
                }

                // ─────────────────────────────────────────────────────────────
                // String operations (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::StringLength => {
                    let s = self.pop_string("string_length")?;
                    #[allow(clippy::cast_precision_loss)]
                    self.stack.push(Value::Number(s.len() as f64));
                }

                Opcode::StringSplit => {
                    let delimiter = self.pop_string("string_split")?;
                    let s = self.pop_string("string_split")?;
                    let parts: Vec<Value> = s
                        .split(&*delimiter)
                        .map(|part| Value::string(part.to_string()))
                        .collect();
                    self.stack.push(Value::list(parts));
                }

                Opcode::StringJoin => {
                    let delimiter = self.pop_string("string_join")?;
                    let list = match self.pop()? {
                        Value::List(elements) => elements,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "list",
                                got: other.type_name(),
                                operation: "string_join",
                            })
                        }
                    };
                    let parts: Vec<String> = list
                        .iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some((**s).clone()),
                            _ => None,
                        })
                        .collect();
                    self.stack.push(Value::string(parts.join(&*delimiter)));
                }

                Opcode::StringTrim => {
                    let s = self.pop_string("string_trim")?;
                    self.stack.push(Value::string(s.trim().to_string()));
                }

                Opcode::StringContains => {
                    let substring = self.pop_string("string_contains")?;
                    let s = self.pop_string("string_contains")?;
                    self.stack.push(Value::Bool(s.contains(&*substring)));
                }

                Opcode::StringConcat => {
                    let s2 = self.pop_string("string_concat")?;
                    let s1 = self.pop_string("string_concat")?;
                    let mut result = (*s1).clone();
                    result.push_str(&s2);
                    self.stack.push(Value::string(result));
                }

                Opcode::StringSlice => {
                    let end = self.pop_number("string_slice")? as usize;
                    let start = self.pop_number("string_slice")? as usize;
                    let s = self.pop_string("string_slice")?;
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len();
                    let start = start.min(len);
                    let end = end.min(len);
                    let result: String = if start >= end {
                        String::new()
                    } else {
                        chars[start..end].iter().collect()
                    };
                    self.stack.push(Value::string(result));
                }

                Opcode::StringChars => {
                    let s = self.pop_string("string_chars")?;
                    let chars: Vec<Value> =
                        s.chars().map(|c| Value::string(c.to_string())).collect();
                    self.stack.push(Value::list(chars));
                }

                Opcode::StringReplace => {
                    let replacement = self.pop_string("string_replace")?;
                    let pattern = self.pop_string("string_replace")?;
                    let s = self.pop_string("string_replace")?;
                    let result = s.replace(&*pattern, &replacement);
                    self.stack.push(Value::string(result));
                }

                Opcode::StringStartsWith => {
                    let prefix = self.pop_string("string_starts_with")?;
                    let s = self.pop_string("string_starts_with")?;
                    self.stack.push(Value::Bool(s.starts_with(&*prefix)));
                }

                Opcode::StringEndsWith => {
                    let suffix = self.pop_string("string_ends_with")?;
                    let s = self.pop_string("string_ends_with")?;
                    self.stack.push(Value::Bool(s.ends_with(&*suffix)));
                }

                Opcode::StringToUpper => {
                    let s = self.pop_string("string_to_upper")?;
                    self.stack.push(Value::string(s.to_uppercase()));
                }

                Opcode::StringToLower => {
                    let s = self.pop_string("string_to_lower")?;
                    self.stack.push(Value::string(s.to_lowercase()));
                }

                Opcode::StringIndexOf => {
                    let substring = self.pop_string("string_index_of")?;
                    let s = self.pop_string("string_index_of")?;
                    let result = match s.find(&*substring) {
                        Some(idx) => {
                            // Convert byte index to character index
                            #[allow(clippy::cast_precision_loss)]
                            let char_idx = s[..idx].chars().count() as f64;
                            char_idx
                        }
                        None => -1.0,
                    };
                    self.stack.push(Value::Number(result));
                }

                Opcode::StringRepeat => {
                    let count = self.pop_number("string_repeat")? as usize;
                    let s = self.pop_string("string_repeat")?;
                    self.stack.push(Value::string(s.repeat(count)));
                }

                Opcode::StringReverse => {
                    let s = self.pop_string("string_reverse")?;
                    let result: String = s.chars().rev().collect();
                    self.stack.push(Value::string(result));
                }

                // ─────────────────────────────────────────────────────────────
                // Type conversion (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::ToString => {
                    let value = self.pop()?;
                    let s = crate::abilities::format_value(&value);
                    self.stack.push(Value::string(s));
                }

                Opcode::ParseNumber => {
                    let s = self.pop_string("parse_number")?;
                    let result = s.trim().parse::<f64>();
                    match result {
                        Ok(n) => {
                            self.stack
                                .push(Value::tuple(vec![Value::Bool(true), Value::Number(n)]));
                        }
                        Err(_) => {
                            self.stack
                                .push(Value::tuple(vec![Value::Bool(false), Value::Number(0.0)]));
                        }
                    }
                }

                Opcode::ParseBool => {
                    let s = self.pop_string("parse_bool")?;
                    let trimmed = s.trim().to_lowercase();
                    let result = match trimmed.as_str() {
                        "true" | "1" | "yes" => Some(true),
                        "false" | "0" | "no" => Some(false),
                        _ => None,
                    };
                    match result {
                        Some(b) => {
                            self.stack
                                .push(Value::tuple(vec![Value::Bool(true), Value::Bool(b)]));
                        }
                        None => {
                            self.stack
                                .push(Value::tuple(vec![Value::Bool(false), Value::Bool(false)]));
                        }
                    }
                }

                // ─────────────────────────────────────────────────────────────
                // Maps (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeEmptyMap => {
                    self.stack.push(Value::empty_map());
                }

                Opcode::MapGet => {
                    let key = self.pop_string("map_get")?;
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_get",
                            })
                        }
                    };
                    let result = map.get(&key).cloned().unwrap_or(Value::Unit);
                    self.stack.push(result);
                }

                Opcode::MapInsert => {
                    let value = self.pop()?;
                    let key = self.pop_string("map_insert")?;
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_insert",
                            })
                        }
                    };
                    let new_map = map.insert(&**key, value);
                    self.stack.push(Value::Map(Arc::new(new_map)));
                }

                Opcode::MapRemove => {
                    let key = self.pop_string("map_remove")?;
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_remove",
                            })
                        }
                    };
                    let new_map = map.remove(&key);
                    self.stack.push(Value::Map(Arc::new(new_map)));
                }

                Opcode::MapContains => {
                    let key = self.pop_string("map_contains")?;
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_contains",
                            })
                        }
                    };
                    self.stack.push(Value::Bool(map.contains_key(&key)));
                }

                Opcode::MapLength => {
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_length",
                            })
                        }
                    };
                    #[allow(clippy::cast_precision_loss)]
                    self.stack.push(Value::Number(map.len() as f64));
                }

                Opcode::MapKeys => {
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_keys",
                            })
                        }
                    };
                    let keys: Vec<Value> = map
                        .keys()
                        .into_iter()
                        .map(|k| Value::String(Arc::new((*k).to_string())))
                        .collect();
                    self.stack.push(Value::list(keys));
                }

                Opcode::MapValues => {
                    let map = match self.pop()? {
                        Value::Map(m) => m,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "map",
                                got: other.type_name(),
                                operation: "map_values",
                            })
                        }
                    };
                    let values = map.values();
                    self.stack.push(Value::list(values));
                }

                // ─────────────────────────────────────────────────────────────
                // Sets (Milestone 15)
                // ─────────────────────────────────────────────────────────────
                Opcode::MakeEmptySet => {
                    self.stack.push(Value::empty_set());
                }

                Opcode::MakeSet => {
                    let count = self.read_u16()? as usize;
                    let mut elements = Vec::with_capacity(count);
                    for _ in 0..count {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    self.stack.push(Value::set(elements));
                }

                Opcode::SetInsert => {
                    let value = self.pop()?;
                    let set = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_insert",
                            })
                        }
                    };
                    let new_set = set.insert(value);
                    self.stack.push(Value::Set(Arc::new(new_set)));
                }

                Opcode::SetRemove => {
                    let value = self.pop()?;
                    let set = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_remove",
                            })
                        }
                    };
                    let new_set = set.remove(&value);
                    self.stack.push(Value::Set(Arc::new(new_set)));
                }

                Opcode::SetContains => {
                    let value = self.pop()?;
                    let set = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_contains",
                            })
                        }
                    };
                    self.stack.push(Value::Bool(set.contains(&value)));
                }

                Opcode::SetLength => {
                    let set = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_length",
                            })
                        }
                    };
                    #[allow(clippy::cast_precision_loss)]
                    self.stack.push(Value::Number(set.len() as f64));
                }

                Opcode::SetUnion => {
                    let set2 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_union",
                            })
                        }
                    };
                    let set1 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_union",
                            })
                        }
                    };
                    let result = set1.union(&set2);
                    self.stack.push(Value::Set(Arc::new(result)));
                }

                Opcode::SetIntersection => {
                    let set2 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_intersection",
                            })
                        }
                    };
                    let set1 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_intersection",
                            })
                        }
                    };
                    let result = set1.intersection(&set2);
                    self.stack.push(Value::Set(Arc::new(result)));
                }

                Opcode::SetDifference => {
                    let set2 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_difference",
                            })
                        }
                    };
                    let set1 = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_difference",
                            })
                        }
                    };
                    let result = set1.difference(&set2);
                    self.stack.push(Value::Set(Arc::new(result)));
                }

                Opcode::SetToList => {
                    let set = match self.pop()? {
                        Value::Set(s) => s,
                        other => {
                            return Err(VmError::TypeError {
                                expected: "set",
                                got: other.type_name(),
                                operation: "set_to_list",
                            })
                        }
                    };
                    self.stack.push(Value::list(set.to_list()));
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
                                expected: "string",
                                got: other.type_name(),
                                operation: "make_enum type_name",
                            })
                        }
                    };

                    // Get the variant name string from constants
                    let variant_name = match self.get_constant(variant_name_idx)? {
                        Value::String(s) => (*s).clone(),
                        other => {
                            return Err(VmError::TypeError {
                                expected: "string",
                                got: other.type_name(),
                                operation: "make_enum variant_name",
                            })
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
                            })
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
                            })
                        }
                    }
                }

                Opcode::EnumTag => {
                    let enum_val = self.pop()?;

                    match enum_val {
                        Value::Enum(e) => {
                            self.stack.push(Value::Number(f64::from(e.tag)));
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "enum",
                                got: other.type_name(),
                                operation: "enum_tag",
                            })
                        }
                    }
                }

                // ─────────────────────────────────────────────────────────────
                // Option/Result utilities
                // ─────────────────────────────────────────────────────────────
                Opcode::OptionUnwrapOr => {
                    // Stack: [option, default] -> [value]
                    let default = self.pop()?;
                    let option = self.pop()?;

                    match option {
                        Value::Enum(e) if &*e.type_name == "Option" => {
                            if e.tag == 1 {
                                // Some(x) - return the payload
                                if let Some(payload) = e.payload.as_deref() {
                                    self.stack.push(payload.clone());
                                } else {
                                    return Err(VmError::EnumPayloadMissing {
                                        type_name: "Option".to_string(),
                                        variant_name: "Some".to_string(),
                                    });
                                }
                            } else {
                                // None - return the default
                                self.stack.push(default);
                            }
                        }
                        other => {
                            return Err(VmError::TypeError {
                                expected: "Option",
                                got: other.type_name(),
                                operation: "option_unwrap_or",
                            })
                        }
                    }
                }

                Opcode::OptionMap => {
                    // Stack: [option, closure] -> [mapped_option]
                    // If Some(x), call f(x) and wrap result in Some
                    // If None, return None
                    let closure = self.pop_closure("option_map")?;
                    let option = self.pop_enum("Option", "option_map")?;
                    self.apply_closure_to_enum(
                        &closure,
                        option,
                        1, // Some tag
                        ReturnAction::WrapSome,
                        "Option",
                        "Some",
                    )?;
                }

                Opcode::OptionAndThen => {
                    // Stack: [option, closure] -> [resulting_option]
                    // If Some(x), call f(x) which returns Option<U>, pass through
                    // If None, return None
                    let closure = self.pop_closure("option_and_then")?;
                    let option = self.pop_enum("Option", "option_and_then")?;
                    self.apply_closure_to_enum(
                        &closure,
                        option,
                        1, // Some tag
                        ReturnAction::PassThrough,
                        "Option",
                        "Some",
                    )?;
                }

                Opcode::ResultMap => {
                    // Stack: [result, closure] -> [mapped_result]
                    // If Ok(x), call f(x) and wrap in Ok
                    // If Err(e), return Err(e)
                    let closure = self.pop_closure("result_map")?;
                    let result_val = self.pop_enum("Result", "result_map")?;
                    self.apply_closure_to_enum(
                        &closure,
                        result_val,
                        0, // Ok tag
                        ReturnAction::WrapOk,
                        "Result",
                        "Ok",
                    )?;
                }

                Opcode::ResultMapErr => {
                    // Stack: [result, closure] -> [mapped_result]
                    // If Ok(x), return Ok(x)
                    // If Err(e), call f(e) and wrap in Err
                    let closure = self.pop_closure("result_map_err")?;
                    let result_val = self.pop_enum("Result", "result_map_err")?;
                    self.apply_closure_to_enum(
                        &closure,
                        result_val,
                        1, // Err tag
                        ReturnAction::WrapErr,
                        "Result",
                        "Err",
                    )?;
                }

                Opcode::ResultAndThen => {
                    // Stack: [result, closure] -> [resulting_result]
                    // If Ok(x), call f(x) which returns Result<U, E>, pass through
                    // If Err(e), return Err(e)
                    let closure = self.pop_closure("result_and_then")?;
                    let result_val = self.pop_enum("Result", "result_and_then")?;
                    self.apply_closure_to_enum(
                        &closure,
                        result_val,
                        0, // Ok tag
                        ReturnAction::PassThrough,
                        "Result",
                        "Ok",
                    )?;
                }
            }
        }
    }
}
