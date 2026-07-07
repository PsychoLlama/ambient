//! [`CompileOptions`] and the `compile_module*` entry points that
//! orchestrate a module compile.

use std::collections::HashMap;
use std::sync::Arc;

use super::context::{FunctionCompiler, ModuleContext, VariantInfo};
use super::error::{CompileError, CompileErrorKind};
use super::expr::compile_expr;
use super::hash::{compute_temporary_hash, finalize_const_values, finalize_module_hashes};
use super::module_output::CompiledModule;
use crate::ast::{FunctionDef, ImplDef, ImplMethod, ItemKind, Module};
use crate::bytecode::{CompiledFunction, Opcode};
use crate::fqn::{Fqn, ModuleId, NameKey};
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
#[derive(Default)]
pub struct CompileOptions<'a> {
    /// The current module's identity. When set, the module's own items
    /// (functions, consts, unit structs, abilities) key on their
    /// [`Fqn`] — matching the resolve pass, which canonicalizes every
    /// same-module reference to `Fqn(module_id, [name])`. `None` is the
    /// registry-less convention (single-file/REPL-less unit compiles): the
    /// resolve pass never ran, so same-module references stay bare and own
    /// items key bare to match.
    pub module_id: Option<ModuleId>,
    /// Original source code, for debug info (line/column mapping).
    pub source: Option<&'a str>,
    /// Source file path, for display in stack traces.
    pub source_file: Option<&'a str>,
    /// Imported function names mapped to their content-addressed hashes.
    pub imported_hashes: Option<HashMap<NameKey, blake3::Hash>>,
    /// Imported enum definitions (`use pkg::m::{SomeEnum}`). Constructors
    /// inline by tag rather than linking by hash, so the compiler needs
    /// the definitions themselves, not name→hash entries.
    pub imported_enums: Vec<crate::ast::EnumDef>,
    /// Foreign unit structs, as canonical `<module>::Origin` keys. A unit
    /// struct compiles to an empty record value, so only its key is needed
    /// (not the definition), keyed like foreign constants. The resolve pass
    /// rewrites every cross-module unit-struct value reference to its
    /// canonical key, which is looked up here.
    pub imported_unit_structs: Vec<NameKey>,
    /// Foreign constant content hashes, keyed by canonical `Fqn`
    /// ([`NameKey::Item`]). A `const` links by hash exactly like a function:
    /// the resolve pass rewrites every cross-module constant reference to its
    /// canonical key, and a reference to a key present here compiles to a
    /// `LoadObject` of that value object (never an inlined literal). The
    /// defining module emits the value object itself, so only the hash is
    /// needed here — a name→hash channel like [`Self::imported_hashes`], not
    /// an AST-replication one.
    pub imported_const_hashes: HashMap<NameKey, blake3::Hash>,
    /// Foreign enum variant constructors, keyed by their canonical
    /// two-segment [`Fqn`] (`core::Option::Some`,
    /// `pkg::shapes::Shape::Circle`). Variant construction inlines a
    /// `MakeEnum` tag rather than linking by hash, so — like
    /// [`Self::imported_enums`] — the compiler needs the tag/payload layout,
    /// not a name→hash entry. This is the *qualified* channel:
    /// [`Self::imported_enums`] covers bare/enum-imported variants;
    /// fully-qualified references resolve to an `Fqn` looked up here.
    pub foreign_enum_variants: Vec<(Fqn, VariantInfo)>,
    /// Prelude abilities (embedder-resolved declaration modules, e.g. the
    /// platform bindings interface). Local declarations shadow them.
    pub prelude_abilities: &'a [std::sync::Arc<crate::ability_resolver::DynAbility>],
    /// Foreign ability identities, keyed by their [`Fqn`]. The resolve pass
    /// rewrites cross-module ability references to these keys; identities
    /// come from the checker (the content-addressed interface hash), never
    /// re-derived here.
    pub foreign_abilities: Vec<(Fqn, std::sync::Arc<crate::ability_resolver::DynAbility>)>,
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
        module_id,
        source,
        source_file,
        imported_hashes,
        imported_enums,
        imported_unit_structs,
        imported_const_hashes,
        foreign_enum_variants,
        prelude_abilities,
        foreign_abilities,
    } = options;
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
    ctx.register_imported_unit_structs(&imported_unit_structs);
    ctx.register_unit_structs(module);
    // Both local (pre-pass) and imported const hashes are final and link the
    // same way; a reference to either key emits a `LoadObject`.
    ctx.register_const_hashes(&local_const_hashes);
    ctx.register_const_hashes(&imported_const_hashes);
    ctx.register_prelude_abilities(prelude_abilities);
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

    // Collect lambda info: (temp_hash, parent_name, compiled_func).
    let lambdas: Vec<(blake3::Hash, Arc<str>, CompiledFunction)> = ctx.lambdas;
    // Value objects for block-scoped consts, discovered while walking bodies.
    let block_const_objects = ctx.const_objects;

    // Phase 3: Compute content-addressed hashes and finalize the module.
    let mut module = finalize_module_hashes(compiled_functions, lambdas)?;
    // Fold in every const value object — module-level ones from the pre-pass
    // and block-scoped ones from the bodies. They ship and deduplicate
    // alongside function objects; a referencing function already records the
    // const hash in its dependencies.
    for (hash, object) in const_objects.into_iter().chain(block_const_objects) {
        module.objects.entry(hash).or_insert(object);
    }
    module.const_names = const_names;
    Ok(module)
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

    // Compile the function body.
    compile_expr(&mut fc, &func.body, ctx)?;

    // Emit return instruction.
    fc.builder.emit(Opcode::Return);

    let param_count = func.params.len() as u8;

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
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
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

    // Compile the method body
    compile_expr(&mut fc, &method.body, ctx)?;

    // Emit return instruction
    fc.builder.emit(Opcode::Return);

    // +1 for the self parameter on instance methods; associated methods
    // (no `self`) take only their declared parameters.
    let param_count = (method.params.len() + usize::from(method.has_self)) as u8;

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
        hash,
        bytecode,
        constants,
        local_count: fc.next_local,
        param_count,
        dependencies,
        debug_info,
    })
}
