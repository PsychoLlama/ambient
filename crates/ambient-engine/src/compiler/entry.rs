//! [`CompileOptions`] and the `compile_module*` entry points that
//! orchestrate a module compile.

use std::collections::HashMap;
use std::sync::Arc;

use super::context::{FunctionCompiler, ModuleContext};
use super::error::{CompileError, CompileErrorKind};
use super::expr::compile_expr_tail;
use super::hash::{compute_temporary_hash, finalize_const_values, finalize_module_hashes};
use super::module_output::CompiledModule;
use crate::ast::{AbilityDef, AbilityMethod, FunctionDef, ImplDef, ImplMethod, ItemKind, Module};
use crate::bytecode::{CompiledFunction, Opcode};
use crate::fqn::{Fqn, ModuleId, NameKey};
use crate::module_env::ModuleEnv;
use crate::value::Value;

/// Helper to convert `Arc<str>` to `Value::String` (which uses `Arc<String>`).
pub(super) fn str_to_value(s: &Arc<str>) -> Value {
    Value::String(Arc::new(s.to_string()))
}

/// Convert a span to (line, column) numbers.
///
/// Line and column are 1-indexed.
pub(super) fn span_to_line_col(source: &str, span: crate::ast::Span) -> (u32, u32) {
    let offset = span.start as usize;
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Options for module compilation.
///
/// The zero-value default compiles without debug info, imports, or
/// prelude abilities.
///
/// Every cross-module channel lives on [`ModuleEnv`], derived from the
/// registry in exactly one place ([`ModuleEnv::new`]) — a registry-backed
/// compile takes the whole env, so it can never see a partial view of the
/// build. Only per-call concerns (debug source, the link table, embedder
/// abilities) are separate fields.
#[derive(Default)]
pub struct CompileOptions<'a> {
    /// Original source code, for debug info (line/column mapping).
    pub source: Option<&'a str>,
    /// Source file path, for display in stack traces.
    pub source_file: Option<&'a str>,
    /// Imported function names mapped to their content-addressed hashes.
    /// Unlike the [`ModuleEnv`] channels this is build-state (hashes of the
    /// modules compiled so far), not derivable from the registry alone.
    pub imported_hashes: Option<HashMap<NameKey, blake3::Hash>>,
    /// The module's resolved view of the rest of the build: its identity,
    /// imported enums, and the foreign variant/unit-struct/const/ability
    /// channels. See [`ModuleEnv`] for what each channel carries.
    pub env: ModuleEnv,
}

/// Compile a module to bytecode.
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module(module: &Module) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, CompileOptions::default())
}

/// Compile a module with explicit [`CompileOptions`].
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module_with_options(
    module: &Module,
    options: CompileOptions,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(module, options)
}

/// Compile a module with imported function references.
///
/// This is used for cross-module compilation where the module imports
/// functions from other already-compiled modules.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `imported_hashes` - Map from imported function names to their content-addressed hashes
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_module_with_imports(
    module: &Module,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            imported_hashes: Some(imported_hashes),
            ..CompileOptions::default()
        },
    )
}

/// Compile a module to bytecode with debug information.
///
/// When `source` and `source_file` are provided, the compiled functions will
/// include debug info mapping bytecode offsets to source locations.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `source` - The original source code (for computing line/column info)
/// * `source_file` - The path to the source file (for display in stack traces)
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
pub fn compile_module_with_source(
    module: &Module,
    source: &str,
    source_file: &str,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            source: Some(source),
            source_file: Some(source_file),
            ..CompileOptions::default()
        },
    )
}

/// Compile a module with imported function references and debug information.
///
/// This combines cross-module compilation with debug info.
///
/// # Arguments
///
/// * `module` - The module to compile
/// * `source` - The original source code (for computing line/column info)
/// * `source_file` - The path to the source file (for display in stack traces)
/// * `imported_hashes` - Map from imported function names to their content-addressed hashes
///
/// # Errors
///
/// Returns a `CompileError` if compilation fails.
#[allow(clippy::implicit_hasher)]
pub fn compile_module_with_imports_and_source(
    module: &Module,
    source: &str,
    source_file: &str,
    imported_hashes: HashMap<NameKey, blake3::Hash>,
) -> Result<CompiledModule, CompileError> {
    compile_module_impl(
        module,
        CompileOptions {
            source: Some(source),
            source_file: Some(source_file),
            imported_hashes: Some(imported_hashes),
            ..CompileOptions::default()
        },
    )
}

/// The lookup key for one of the current module's own items (function,
/// const, unit struct), matching the resolve pass: `Item(Fqn(module_id,
/// [name]))` when the module has an identity, else bare (registry-less).
pub(super) fn own_item_key(module_id: Option<&ModuleId>, name: &Arc<str>) -> NameKey {
    match module_id {
        Some(id) => NameKey::Item(Fqn::new(id.clone(), vec![Arc::clone(name)])),
        None => NameKey::Bare(Arc::clone(name)),
    }
}

/// Implementation of module compilation with optional debug info.
#[allow(clippy::too_many_lines)]
fn compile_module_impl(
    module: &Module,
    options: CompileOptions,
) -> Result<CompiledModule, CompileError> {
    let CompileOptions {
        source,
        source_file,
        imported_hashes,
        env,
    } = options;
    let ModuleEnv {
        module_id,
        imported_enums,
        foreign_enum_variants,
        foreign_unit_structs,
        foreign_const_hashes,
        foreign_abilities,
        extern_natives,
    } = env;
    // Collect function definitions.
    let functions: Vec<&FunctionDef> = module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Function(func) = &item.kind {
                Some(func)
            } else {
                None
            }
        })
        .collect();

    // Phase 1: Create temporary hashes for name-based lookup during compilation.
    // These will be replaced with content-addressed hashes after compilation.
    // Start with imported hashes (these are already content-addressed).
    let mut temp_hashes: HashMap<NameKey, blake3::Hash> = imported_hashes.unwrap_or_default();

    // Add temporary hashes for local functions, keyed on the same identity
    // the resolve pass gives a same-module reference: `Item(Fqn)` when the
    // module has an identity, bare in the registry-less convention.
    for func in &functions {
        let hash = compute_temporary_hash(&func.name);
        temp_hashes.insert(own_item_key(module_id.as_ref(), &func.name), hash);
    }

    // Content-address every `extern fn` in a pre-pass, like consts: a
    // native object is a leaf — a pure function of its host binding's
    // `(uuid, param_count)` — so its hash is final immediately and function
    // bodies compiled afterward link to it like any callee. The declared
    // name never enters the object, so renaming an extern fn moves no hash;
    // it only re-keys the host binding (a loud error here until updated).
    let mut native_objects: HashMap<blake3::Hash, crate::object::StoredObject> = HashMap::new();
    let mut native_names: HashMap<Arc<str>, blake3::Hash> = HashMap::new();
    for item in &module.items {
        let ItemKind::ExternFn(def) = &item.kind else {
            continue;
        };
        let span = (def.name_span.start, def.name_span.end);
        let Some(key) = extern_natives.get(&def.name) else {
            return Err(CompileError::new(
                CompileErrorKind::UnboundExternFn {
                    name: Arc::clone(&def.name),
                },
                span,
            ));
        };
        let declared = def.params.len() as u8;
        if key.arity != declared {
            return Err(CompileError::new(
                CompileErrorKind::ExternArityMismatch {
                    name: Arc::clone(&def.name),
                    declared,
                    bound: key.arity,
                },
                span,
            ));
        }
        let object = crate::object::StoredObject::Native {
            uuid: key.uuid,
            param_count: declared,
        };
        let hash = object.hash();
        temp_hashes.insert(own_item_key(module_id.as_ref(), &def.name), hash);
        native_objects.insert(hash, object);
        native_names.insert(Arc::clone(&def.name), hash);
    }

    // Content-address every local `const` in a pre-pass, before function
    // bodies compile, so a reference can link to the value object's final
    // hash (see `finalize_const_values`). A `const` is not compiled to a
    // function — it is a standalone leaf value object — so it needs no temp
    // hash or call-graph pass.
    let local_consts: Vec<(NameKey, Value)> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Const(const_def) => {
                let value = crate::const_eval::literal_value(&const_def.value)?;
                Some((own_item_key(module_id.as_ref(), &const_def.name), value))
            }
            _ => None,
        })
        .collect();
    let (local_const_hashes, const_objects) = finalize_const_values(&local_consts);

    // Bind each local const's short name to its value-object hash, so a
    // named const is a first-class binding in the store's `names` index
    // (imported consts are named in their own module).
    let const_names: HashMap<Arc<str>, blake3::Hash> = module
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::Const(const_def) => {
                let key = own_item_key(module_id.as_ref(), &const_def.name);
                let hash = local_const_hashes.get(&key)?;
                Some((Arc::clone(&const_def.name), *hash))
            }
            _ => None,
        })
        .collect();

    // Ability default implementations compile as ordinary functions under
    // their content dispatch symbols (`<ability-uuid>::<method>`), exactly
    // like impl methods: perform sites resolve the symbol through the same
    // name→hash table as regular calls, and hash finalization
    // content-addresses them with everything else. The declaring module's
    // exports carry them (the `::` keeps them `NameKey::Bare` in linking),
    // so foreign perform sites link to them like any function.
    let ability_methods: Vec<(&AbilityDef, &AbilityMethod, Arc<str>)> = module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Ability(def) = &item.kind {
                Some(
                    def.methods
                        .iter()
                        .filter(|m| m.body.is_some())
                        .map(move |m| {
                            let symbol: Arc<str> = Arc::from(format!("{}::{}", def.uuid, m.name));
                            (def, m, symbol)
                        }),
                )
            } else {
                None
            }
        })
        .flatten()
        .collect();

    for (_, _, symbol) in &ability_methods {
        temp_hashes.insert(
            NameKey::Bare(Arc::clone(symbol)),
            compute_temporary_hash(symbol),
        );
    }

    // Add temporary hashes for impl methods under their canonical symbols.
    // Impl methods are ordinary functions named by `types::impl_method_symbol`;
    // method-call sites resolve these symbols through the same name→hash table
    // as regular calls, and hash finalization content-addresses them together
    // with everything else.
    let impl_methods: Vec<(&ImplDef, &ImplMethod)> = module
        .items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Impl(impl_def) = &item.kind {
                Some(impl_def.methods.iter().map(move |m| (impl_def, m)))
            } else {
                None
            }
        })
        .flatten()
        .collect();

    for (_, method) in &impl_methods {
        let Some(symbol) = &method.resolved_symbol else {
            return Err(CompileError::new(
                CompileErrorKind::Internal {
                    message: "impl method missing resolved symbol",
                },
                (method.span.start, method.span.end),
            ));
        };
        temp_hashes.insert(
            NameKey::Bare(Arc::clone(symbol)),
            compute_temporary_hash(symbol),
        );
    }

    // Create module context for tracking lambdas discovered during
    // compilation, with the module's enum constructors registered.
    let mut ctx = ModuleContext::new(module_id.clone());
    ctx.register_imported_enums(&imported_enums);
    ctx.register_enums(module);
    ctx.register_foreign_variants(&foreign_enum_variants);
    ctx.register_imported_unit_structs(&foreign_unit_structs);
    ctx.register_unit_structs(module);
    // Both local (pre-pass) and foreign const hashes are final and link the
    // same way; a reference to either key emits a `LoadObject`.
    ctx.register_const_hashes(&local_const_hashes);
    ctx.register_const_hashes(&foreign_const_hashes);
    ctx.register_foreign_abilities(&foreign_abilities);
    ctx.register_abilities(module)?;

    // Phase 2: Compile each function using temporary hashes.
    let mut compiled_functions: Vec<(Arc<str>, CompiledFunction, bool)> = Vec::new();
    for func in &functions {
        // Track current function for lambda parent relationships.
        ctx.set_current_function(Arc::clone(&func.name));
        let compiled =
            compile_function_with_hash(func, &temp_hashes, &mut ctx, source, source_file)?;
        let is_main = &*func.name == "run";
        compiled_functions.push((Arc::clone(&func.name), compiled, is_main));
    }

    // Constants compile to standalone value objects (folded into the module
    // below), not functions; each reference is a `LoadObject` of the object's
    // hash (see the `ExprKind::Name` arm).

    // Compile impl methods as ordinary named functions.
    for (impl_def, method) in &impl_methods {
        // Presence was validated when building temp_hashes above.
        let Some(symbol) = &method.resolved_symbol else {
            continue;
        };
        ctx.set_current_function(Arc::clone(symbol));
        let compiled = compile_impl_method(
            impl_def,
            method,
            &temp_hashes,
            &mut ctx,
            source,
            source_file,
        )?;
        compiled_functions.push((Arc::clone(symbol), compiled, false));
    }

    // Compile ability default implementations as ordinary named functions.
    for (def, method, symbol) in &ability_methods {
        ctx.set_current_function(Arc::clone(symbol));
        let compiled = compile_ability_method(
            def,
            method,
            symbol,
            &temp_hashes,
            &mut ctx,
            source,
            source_file,
        )?;
        compiled_functions.push((Arc::clone(symbol), compiled, false));
    }

    // Collect lambda info: (temp_hash, parent_name, compiled_func).
    let lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)> = ctx.lambdas;
    // Value objects for block-scoped consts, discovered while walking bodies.
    let block_const_objects = ctx.const_objects;
    // Statically-named state migrations, for pre-swap deploy validation.
    let migrations = ctx.migrations;

    // An ability default implementation compiles under the dispatch symbol
    // `<ability-uuid>::<method>`, which carries the method name. If such an
    // impl lands in a recursive group object, group members are identified
    // by name — so the method name would leak into the group hash, hence the
    // impl's content hash, hence its rename-stable `MethodKey`. Map each
    // impl symbol to a rename-stable canonical group name (the ability uuid
    // alone) so finalization identifies the member by uuid, not method name.
    let ability_impl_group_names: HashMap<Arc<str>, Arc<str>> = ability_methods
        .iter()
        .map(|(def, _, symbol)| (Arc::clone(symbol), Arc::from(def.uuid.to_string())))
        .collect();

    // Phase 3: Compute content-addressed hashes and finalize the module.
    let mut module =
        finalize_module_hashes(compiled_functions, lambdas, &ability_impl_group_names)?;
    module.migrations = migrations;
    // Fold in every const value object — module-level ones from the pre-pass
    // and block-scoped ones from the bodies. They ship and deduplicate
    // alongside function objects; a referencing function already records the
    // const hash in its dependencies.
    for (hash, object) in const_objects.into_iter().chain(block_const_objects) {
        module.objects.entry(hash).or_insert(object);
    }
    module.const_names = const_names;
    // Fold in the module's extern fns. They bind names like functions (an
    // extern fn exports, imports, and links exactly like a compiled one),
    // and their native objects ship alongside everything else.
    for (hash, object) in native_objects {
        module.objects.entry(hash).or_insert(object);
    }
    for (name, hash) in native_names {
        module.function_names.entry(name).or_insert(hash);
    }

    // A method's identity is (ability uuid, signature, implementation) —
    // the name is deliberately excluded. Two methods of one ability with
    // the same signature *and* the same default implementation would
    // therefore derive one `MethodKey`: handler arms for them silently
    // merge and performs become indistinguishable. Reject the ambiguity
    // now that final implementation hashes exist.
    {
        let mut seen: HashMap<(uuid::Uuid, ambient_core::SignatureHash, blake3::Hash), Arc<str>> =
            HashMap::new();
        for (def, method, symbol) in &ability_methods {
            let (Some(signature), Some(impl_hash)) = (
                method.resolved_signature,
                module.function_names.get(symbol.as_ref()),
            ) else {
                continue;
            };
            if let Some(previous) =
                seen.insert((def.uuid, signature, *impl_hash), Arc::clone(&method.name))
            {
                return Err(CompileError::new(
                    CompileErrorKind::Unsupported {
                        feature: format!(
                            "ability `{}` methods `{previous}` and `{}` share a signature \
                             and an identical default implementation, so they would be one \
                             method at runtime; make the implementations differ",
                            def.name, method.name
                        ),
                    },
                    (method.span.start, method.span.end),
                ));
            }
        }
    }

    Ok(module)
}

/// Allocate the hidden trailing dictionary parameters a bounded item takes:
/// one local per entry of [`crate::ast::dict_params`] — the same authority
/// the checker orders scheme bounds and call-site dictionaries by, so slots
/// and indices can never disagree. The locals have no source `BindingId`;
/// `fc.dict_locals` records their slots for arity, and each is also bound
/// under an index-keyed pseudo-name ([`super::context::dict_capture_name`])
/// so dictionary-slot dispatch and forwarding reach them through the
/// ordinary name-capture path — the same one that threads them into lambdas.
fn alloc_dict_locals(
    fc: &mut FunctionCompiler,
    type_params: &[crate::ast::TypeParam],
) -> Result<(), CompileError> {
    for (index, (param, bound)) in crate::ast::dict_params(type_params).into_iter().enumerate() {
        let slot = fc.next_local;
        if slot == u16::MAX {
            return Err(CompileError::new(
                CompileErrorKind::TooManyLocals {
                    count: slot as usize + 1,
                },
                (0, 0),
            ));
        }
        fc.next_local += 1;
        fc.record_local_name(slot, &format!("<dict {param}: {}>", bound.name));
        fc.local_names
            .insert(super::context::dict_capture_name(index), slot);
        fc.dict_locals.push(slot);
    }
    Ok(())
}

/// Compile a function with pre-determined hash.
pub(super) fn compile_function_with_hash(
    func: &FunctionDef,
    function_hashes: &HashMap<NameKey, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slots for parameters (with names for lookup).
    for param in &func.params {
        fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Hidden trailing dictionary parameters, one per trait bound.
    alloc_dict_locals(&mut fc, &func.type_params)?;

    // Compile the function body. A function body is in tail position: a call
    // that is its final act becomes a frame-reusing tail call, followed by
    // the unconditional trailing `Return` (dead after a tail call, harmless).
    compile_expr_tail(&mut fc, &func.body, ctx, true)?;

    // Emit return instruction.
    fc.builder.emit(Opcode::Return);

    let param_count = (func.params.len() + fc.dict_locals.len()) as u8;

    // Build with the pre-computed dependencies from the builder
    let bytecode = fc.builder.bytecode().to_vec();
    let constants = fc.builder.constants().to_vec();
    let dependencies = fc.builder.dependencies().to_vec();

    // Get the pre-computed hash for this function, keyed like its
    // same-module references (see `own_item_key`).
    let hash = function_hashes[&own_item_key(ctx.module_id.as_ref(), &func.name)];

    // Build debug info if source is available
    let debug_info = if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(func.name.to_string());
        debug_info.source_file = source_file.map(String::from);

        // Compute line/column numbers from source spans
        if let Some(src) = source {
            for mapping in &mut debug_info.source_map {
                let (line, col) = span_to_line_col(
                    src,
                    crate::ast::Span::new(mapping.source_start as u32, mapping.source_end as u32),
                );
                mapping.line = line;
                mapping.column = col;
            }
        }

        Some(debug_info)
    } else {
        None
    };

    Ok(CompiledFunction {
        method_keys: CompiledFunction::index_method_keys(&constants),
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info,
    })
}

/// Compile one ability method's default implementation as an ordinary
/// function under its dispatch symbol. This is the function an unhandled
/// perform calls, and its content hash is an input to the method's
/// `MethodKey`.
fn compile_ability_method(
    def: &AbilityDef,
    method: &AbilityMethod,
    symbol: &Arc<str>,
    function_hashes: &HashMap<NameKey, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    for param in &method.params {
        fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Hidden trailing dictionary parameters, one per trait bound on the
    // method's type parameters — perform sites push them after the
    // declared arguments.
    alloc_dict_locals(&mut fc, &method.type_params)?;

    let Some(body) = &method.body else {
        return Err(CompileError::new(
            CompileErrorKind::Internal {
                message: "ability method without a body reached compilation",
            },
            (method.span.start, method.span.end),
        ));
    };
    // The default implementation's body is in tail position, like any
    // function body.
    compile_expr_tail(&mut fc, body, ctx, true)?;
    fc.builder.emit(Opcode::Return);

    let hash = function_hashes[&NameKey::Bare(Arc::clone(symbol))];

    let debug_info = if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(format!("{}::{}", def.name, method.name));
        debug_info.source_file = source_file.map(String::from);
        if let Some(src) = source {
            for mapping in &mut debug_info.source_map {
                let (line, col) = span_to_line_col(
                    src,
                    crate::ast::Span::new(mapping.source_start as u32, mapping.source_end as u32),
                );
                mapping.line = line;
                mapping.column = col;
            }
        }
        Some(debug_info)
    } else {
        None
    };

    let constants = fc.builder.constants().to_vec();
    Ok(CompiledFunction {
        method_keys: CompiledFunction::index_method_keys(&constants),
        hash,
        bytecode: fc.builder.bytecode().to_vec(),
        constants,
        local_count: fc.next_local,
        // Dictionary parameters count toward the arity: perform sites push
        // them after the declared arguments.
        #[allow(clippy::cast_possible_truncation)]
        param_count: (method.params.len() + fc.dict_locals.len()) as u8,
        dependencies: fc.builder.dependencies().to_vec(),
        debug_info,
    })
}

/// Compile a single impl method as an ordinary function.
///
/// The method is registered in the module under its canonical symbol
/// (`types::impl_method_symbol`), so it participates in content-addressed
/// hash finalization exactly like a named function.
fn compile_impl_method(
    impl_def: &ImplDef,
    method: &ImplMethod,
    function_hashes: &HashMap<NameKey, blake3::Hash>,
    ctx: &mut ModuleContext,
    source: Option<&str>,
    source_file: Option<&str>,
) -> Result<CompiledFunction, CompileError> {
    let mut fc = FunctionCompiler::new(function_hashes.clone());

    // Allocate slot for the self parameter. Associated methods (e.g.
    // `Default::default`) take no `self`, so the slot and its argument are
    // omitted for them.
    if method.has_self {
        let self_name: Arc<str> = "self".into();
        fc.alloc_local_with_name(method.self_id, &self_name)?;
    }

    // Allocate slots for other parameters
    for param in &method.params {
        fc.alloc_local_with_name(param.id, &param.name)?;
    }

    // Hidden trailing dictionary parameters: the impl block's bounds first,
    // then the method's own — the same combined order the checker built the
    // method's scheme bounds in.
    let combined_params: Vec<crate::ast::TypeParam> = impl_def
        .type_params
        .iter()
        .chain(method.type_params.iter())
        .cloned()
        .collect();
    alloc_dict_locals(&mut fc, &combined_params)?;

    // Compile the method body, which is in tail position like any function
    // body.
    compile_expr_tail(&mut fc, &method.body, ctx, true)?;

    // Emit return instruction
    fc.builder.emit(Opcode::Return);

    // +1 for the self parameter on instance methods; associated methods
    // (no `self`) take only their declared parameters. Dictionary
    // parameters count toward the arity: call sites push them.
    let param_count =
        (method.params.len() + usize::from(method.has_self) + fc.dict_locals.len()) as u8;

    let bytecode = fc.builder.bytecode().to_vec();
    let constants = fc.builder.constants().to_vec();
    let dependencies = fc.builder.dependencies().to_vec();

    // The temporary hash; finalization replaces it with the content hash.
    let hash = method
        .resolved_symbol
        .as_ref()
        .and_then(|symbol| function_hashes.get(&NameKey::Bare(Arc::clone(symbol))))
        .copied()
        .ok_or_else(|| {
            CompileError::new(
                CompileErrorKind::Internal {
                    message: "impl method missing resolved symbol",
                },
                (method.span.start, method.span.end),
            )
        })?;

    // Build debug info if source is available
    let debug_info = if source.is_some() || source_file.is_some() {
        let mut debug_info = fc.debug_info;
        debug_info.function_name = Some(match &impl_def.trait_name {
            Some(trait_name) => format!("{}::{}", trait_name.name, method.name),
            None => format!("{}::{}", impl_def.for_type, method.name),
        });
        debug_info.source_file = source_file.map(String::from);

        // Compute line/column numbers from source spans
        if let Some(src) = source {
            for mapping in &mut debug_info.source_map {
                let (line, col) = span_to_line_col(
                    src,
                    crate::ast::Span::new(mapping.source_start as u32, mapping.source_end as u32),
                );
                mapping.line = line;
                mapping.column = col;
            }
        }

        Some(debug_info)
    } else {
        None
    };

    Ok(CompiledFunction {
        method_keys: CompiledFunction::index_method_keys(&constants),
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info,
    })
}
