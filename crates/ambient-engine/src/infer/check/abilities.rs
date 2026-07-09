//! Ability seeding and resolution: prelude primitive aliases, namespaced
//! dynamics, `ability` declarations, and the embedder entry points.

use std::collections::HashMap;
use std::sync::Arc;

use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;
use crate::types::Type;

use crate::infer::Infer;
use crate::infer::error::{BoxedTypeError, TypeError, TypeErrorKind};

use super::locals::substitute_type_params;

/// Seed the prelude's `extern` struct types into the alias table: the four
/// primitive nominals (`Bool`/`Number`/`String`/`Binary`) and the opaque
/// generic containers (`List`/`Map`/`Set`).
///
/// Ability resolution runs on an `Infer::new()` with no import processing, so
/// `resolve_holes` has no way to turn a bare primitive or container name in a
/// signature into its uuid-carrying form — which the canonical renderer needs,
/// or the ability hash drifts. This threads exactly the prelude's `extern`
/// structs in through the module system
/// ([`ModuleRegistry::prelude_struct_defs`]), registered by the same
/// [`AliasTarget::of_struct`] rule as every other channel, leaving every
/// other named type (`Duration`, `Option`) untouched so their renderings
/// stay byte-identical.
pub(super) fn seed_prelude_struct_aliases(infer: &mut Infer, registry: &ModuleRegistry) {
    for (name, def) in registry.prelude_struct_defs() {
        infer.register_type_alias_target(name, crate::infer::AliasTarget::of_struct(&def));
    }
}

/// The inference context every ability-id-computing path starts from: a
/// fresh `Infer` with the prelude's `extern` struct types seeded and
/// nothing else.
///
/// Ability identity is the hash of the canonically rendered interface, so
/// every path that computes one must resolve type names identically.
/// Constructing the context here — instead of each entry point remembering
/// to seed — makes a fourth path that forgets impossible to write by
/// copying an existing one.
fn ability_id_infer(registry: &ModuleRegistry) -> Infer {
    let mut infer = Infer::new();
    seed_prelude_struct_aliases(&mut infer, registry);
    infer
}
/// Register the module's `ability` declarations.
///
/// Each declaration's method signatures are resolved (type parameters
/// become quantified type variables, aliases expand), rendered to
/// canonical form, and hashed into the ability's content-addressed
/// identity. The resulting [`DynAbility`] joins the resolver so
/// perform/suspend/handle and `with` clauses see it exactly like a
/// builtin; the computed identity is stored back into the AST for the
/// compiler.
///
/// Declared dependencies (a local `ability B with A`) resolve against
/// abilities already known to the resolver — builtins or dynamics
/// registered earlier in the item list — and are recorded in the ability
/// registry so requiring the ability transitively requires them.
pub(super) fn register_abilities(
    infer: &mut Infer,
    module: &mut crate::ast::Module,
    errors: &mut Vec<BoxedTypeError>,
) -> Vec<Arc<crate::ability_resolver::DynAbility>> {
    let mut resolved = Vec::new();
    for item in &mut module.items {
        let crate::ast::ItemKind::Ability(def) = &mut item.kind else {
            continue;
        };

        // Every ability method carries a default implementation — the
        // behavior of an unhandled perform. The one carve-out is the
        // reserved Exception declaration: `throw` returns `!`, and its
        // unhandled behavior is the VM's own uncaught-exception path,
        // which no in-language body could express.
        if def.uuid != ambient_core::exception::EXCEPTION_UUID {
            for method in &def.methods {
                if method.body.is_none() {
                    errors.push(Box::new(TypeError::new(
                        TypeErrorKind::InvalidDeclaration {
                            message: format!(
                                "ability method `{}::{}` needs a default implementation: \
                                 the body is what an unhandled perform runs \
                                 (`fn {}(...): T {{ ... }}`)",
                                def.name, method.name, method.name
                            ),
                        },
                        (method.span.start, method.span.end),
                    )));
                }
            }
        }

        let dyn_ab = resolve_ability_def(infer, def, errors);
        // The compiler reads the identity and per-method signature hashes
        // back from the AST.
        def.resolved_id = Some(dyn_ab.id);
        for (method, resolved_method) in def.methods.iter_mut().zip(&dyn_ab.methods) {
            method.resolved_signature = Some(resolved_method.signature);
        }
        infer.ability_resolver.register_dynamic(dyn_ab);
        if let Some(ability) = infer.ability_resolver.get_dynamic(&def.name) {
            resolved.push(Arc::clone(ability));
        }
    }
    resolved
}
/// Register a cross-module ability import (`use pkg::b::SomeAbility;`,
/// `use core::system::Network;`) as a *bare* local dynamic, resolved from
/// the origin module's declaration.
///
/// The identity is content-addressed, so it unifies with the origin
/// module's own registration — and with any namespaced copy
/// (`core::system::Network`) — meaning handlers, effect-rows, and linking
/// need no changes. Called from `build_import_env` for each `ExportKind::Ability`
/// import.
pub(super) fn register_imported_ability(
    infer: &mut Infer,
    registry: &ModuleRegistry,
    from_module: &ModulePath,
    name: &str,
    errors: &mut Vec<BoxedTypeError>,
) {
    let Some(module_info) = registry.get(from_module) else {
        return;
    };
    let Some(def) = module_info
        .module
        .items
        .iter()
        .find_map(|item| match &item.kind {
            crate::ast::ItemKind::Ability(def) if def.name.as_ref() == name => Some(def),
            _ => None,
        })
    else {
        return;
    };
    let dyn_ab = resolve_ability_def(infer, def, errors);
    infer.ability_resolver.register_dynamic(dyn_ab);
}
/// Seed every registered module's `ability` declarations as namespaced
/// dynamics under the declaring module's dotted path
/// (`core::system.Network`, `effects.Counter`, `deep.nested.fx.Log`).
///
/// This is the ability-layer counterpart of canonical name resolution:
/// the resolve pass rewrites every qualified or imported ability
/// reference to `<declaring module>::<Ability>`, and this seeding is what
/// makes that namespace resolvable — on every checking path that has a
/// registry (single-file, package, and LSP). Because ability identity is
/// the content-addressed interface hash, seeding is deterministic and a
/// bare local registration of the same declaration unifies with it.
///
/// Every module seeds — including the current one, whose declarations
/// *also* register bare in `register_abilities` (locals stay bare;
/// references to them normalize to the bare form). Seeding the current
/// module's namespace matters for hydrating foreign signatures: checking
/// `effects` hydrates `worker.tick`, whose `with` clause resolved to
/// `effects::Counter`. The `core::system` module seeds first so its
/// intra-file dependencies (`Log with core::system::Stdio`) resolve; other
/// modules seed in path order. Resolution errors inside *foreign* modules
/// are not this module's diagnostics — they surface when that module
/// itself is checked — except for `core::system`, whose declarations have
/// no other checking path.
pub(super) fn seed_namespaced_ability_dynamics(
    infer: &mut Infer,
    registry: &ModuleRegistry,
    errors: &mut Vec<BoxedTypeError>,
) {
    let mut modules: Vec<_> = registry
        .all_modules()
        .map(|info| (info.path.clone(), Arc::clone(&info.module)))
        .collect();
    modules.sort_by_key(|(path, _)| {
        // The declaration module (`core::system`) first, then path order.
        let key = path.to_string();
        (key != "core::system", key)
    });

    let core_system = crate::fqn::ModuleId::core_system();
    for (path, module) in modules {
        let namespace = registry.module_id(&path);
        let is_declaration = namespace == core_system;
        let mut foreign_errors = Vec::new();
        for item in &module.items {
            if let crate::ast::ItemKind::Ability(def) = &item.kind {
                let dyn_ab = resolve_ability_def(infer, def, &mut foreign_errors);
                infer
                    .ability_resolver
                    .register_dynamic_in_namespace(&namespace, dyn_ab);
            }
        }
        if is_declaration {
            errors.append(&mut foreign_errors);
        }
    }
}
/// Find a residual bare primitive name (`Bool`/`Number`/`String`/`Binary`)
/// anywhere in `ty` — an argument-less, uuid-less `Named` whose name is a
/// primitive. Recurses into type arguments so a nested `List<String>` is
/// caught too. Used only as the ability-hash tripwire below.
fn residual_primitive_name(ty: &Type) -> Option<&str> {
    match ty {
        Type::Named(named) => {
            if named.args.is_empty()
                && named.uuid.is_none()
                && crate::types::Primitive::from_name(&named.name).is_some()
            {
                return Some(&named.name);
            }
            named.args.iter().find_map(residual_primitive_name)
        }
        _ => None,
    }
}

/// Resolve one `ability` declaration into a [`DynAbility`], recording its
/// transitive dependencies in the ability registry.
///
/// Shared by the local path ([`register_abilities`], which additionally
/// writes the identity and per-method signature hashes back into the AST
/// and registers it *bare*) and the cross-module import path
/// ([`build_import_env`], which registers the result bare from a foreign
/// module's declaration). The identity is the declaration uuid, and the
/// canonical signature renderings are deterministic, so a foreign import
/// matches the origin module's own registration without touching the
/// (immutable) foreign AST.
fn resolve_ability_def(
    infer: &mut Infer,
    def: &crate::ast::AbilityDef,
    errors: &mut Vec<BoxedTypeError>,
) -> crate::ability_resolver::DynAbility {
    use crate::ability_resolver::{CanonicalTypeRenderer, DynAbility, DynMethod};
    use ambient_core::SignatureHash;

    // Resolve dependencies first: they must already be known. The
    // namespace policy applies here too: `ability Log with
    // core::system::Stdio` — a system dependency needs its prefix.
    let mut dependencies = Vec::new();
    for dep in &def.dependencies {
        match infer.resolve_ability_ref(dep, (def.name_span.start, def.name_span.end)) {
            Ok(id) => dependencies.push(id),
            Err(e) => errors.push(e),
        }
    }

    let mut methods = Vec::new();
    for method in &def.methods {
        // Type parameters become quantified variables, substituted
        // into the declared types.
        let mut param_map = HashMap::new();
        let mut quantified = Vec::new();
        for tp in &method.type_params {
            let var_id = infer.r#gen.fresh_id();
            param_map.insert(Arc::clone(&tp.name), var_id);
            quantified.push(var_id);
        }

        // `resolve_holes` resolves bare primitive names to their nominal
        // primitive type (a builtin identity, context-independent), so the
        // canonical interface renders `number`/`string`/... rather than the
        // `named:Number` an unresolved name would produce — keeping ability
        // identities byte-stable regardless of the prelude/imports in scope.
        // Plain `resolve_holes` (not `resolve_erroring`) here: an ability
        // signature is resolved *before* the module's type-alias table is
        // populated, so a cross-module nominal (`Duration`) legitimately
        // stays an unresolved `Named` — bridged to the real type at use
        // sites and rendered as `named:Duration` for hashing. Rewriting it to
        // `Type::Error` would break both the bridge and hash stability. Typos
        // in a *local* ability's signature are still reported by the
        // declared-types sweep (which runs with the alias table populated).
        let params: Vec<Type> = method
            .params
            .iter()
            .map(|p| infer.resolve_holes(&substitute_type_params(p.declared_ty(), &param_map)))
            .collect();
        let ret = infer.resolve_holes(&substitute_type_params(&method.ret_ty, &param_map));

        // Tripwire: a primitive that stayed a bare `Named` (uuid-less, no
        // args) after `resolve_holes` would render `named:String` and
        // silently corrupt this ability's hash — the exact regression that
        // deleting the `Primitive::from_name` shortcut could reintroduce if
        // the prelude primitives ever stop being seeded. Fail loudly instead
        // of hashing wrong. (Non-primitive names like `Duration` legitimately
        // stay unresolved and are byte-stable, so they are not flagged.)
        for ty in params.iter().chain(std::iter::once(&ret)) {
            if let Some(name) = residual_primitive_name(ty) {
                errors.push(Box::new(TypeError::new(
                    TypeErrorKind::InvalidDeclaration {
                        message: format!(
                            "primitive `{name}` in ability `{}` resolved to a bare name; \
                             the prelude primitive nominals were not seeded — this would \
                             corrupt the ability hash",
                            def.name
                        ),
                    },
                    (def.name_span.start, def.name_span.end),
                )));
            }
        }

        // One renderer per signature: variable numbering is
        // signature-local, by first occurrence.
        let mut renderer = CanonicalTypeRenderer::new();
        let canon_params: Vec<String> = params.iter().map(|p| renderer.render(p)).collect();
        let canon_ret = renderer.render(&ret);

        methods.push(DynMethod {
            name: Arc::clone(&method.name),
            param_names: method.params.iter().map(|p| Arc::clone(&p.name)).collect(),
            params,
            ret,
            quantified,
            signature: SignatureHash::new(&canon_params, &canon_ret),
            has_impl: method.body.is_some(),
        });
    }

    // Nominal identity: the declaration uuid is the ability, so renames
    // and moves never change it and same-shaped declarations never unify.
    let id = crate::types::AbilityId::from_uuid(&def.uuid);

    // Record dependencies so `require_ability` pulls them transitively.
    if !dependencies.is_empty() {
        let registry = infer
            .ability_registry
            .get_or_insert_with(crate::types::AbilityRegistry::new);
        let mut info = crate::types::AbilityInfo::new(def.name.as_ref());
        for dep in &dependencies {
            info = info.with_dependency(*dep);
        }
        registry.register(id, info);
    }

    DynAbility {
        id,
        uuid: def.uuid,
        name: Arc::clone(&def.name),
        methods,
        dependencies,
    }
}
/// Resolve a module's `ability` declarations without checking the rest of
/// the module.
///
/// This is the entry point for **ability preludes**: an embedder parses a
/// module containing only `ability` declarations (e.g. the platform
/// bindings interface), resolves them here, and registers the results as
/// namespaced dynamics on the resolver it threads into checking — and as
/// the identity/method-id source when binding host handlers on the VM.
///
/// Each declaration's `resolved_id` is written back into the AST, exactly
/// as during a full module check.
pub fn resolve_ability_declarations(
    module: &mut crate::ast::Module,
    registry: &ModuleRegistry,
) -> (
    Vec<Arc<crate::ability_resolver::DynAbility>>,
    Vec<BoxedTypeError>,
) {
    // A primitive named in an ability signature must resolve to its
    // uuid-carrying type or the rendering (and so the ability hash) drifts;
    // `ability_id_infer` seeds exactly that and nothing else, so
    // `Duration`/`Option` stay byte-identical to before.
    let mut infer = ability_id_infer(registry);
    let mut errors = Vec::new();

    // Register each declaration under the `core::system` namespace
    // *before* resolving the next, so an intra-module dependency
    // (`ability Log with core::system::Stdio`) resolves exactly as it does
    // when checking user code (see `seed_namespaced_ability_dynamics`,
    // which also hardcodes `core::system`). Registering these bare — as the
    // local module path does — would leave a `core::system::`-qualified
    // dependency unresolvable.
    let mut abilities = Vec::new();
    for item in &mut module.items {
        let crate::ast::ItemKind::Ability(def) = &mut item.kind else {
            continue;
        };
        let dyn_ab = resolve_ability_def(&mut infer, def, &mut errors);
        // The compiler reads the identity and signatures back from the AST.
        def.resolved_id = Some(dyn_ab.id);
        for (method, resolved_method) in def.methods.iter_mut().zip(&dyn_ab.methods) {
            method.resolved_signature = Some(resolved_method.signature);
        }
        let core_system = crate::fqn::ModuleId::core_system();
        infer
            .ability_resolver
            .register_dynamic_in_namespace(&core_system, dyn_ab);
        if let Some(ability) = infer
            .ability_resolver
            .get_namespaced(&core_system, &def.name)
        {
            abilities.push(Arc::clone(ability));
        }
    }
    (abilities, errors)
}
/// Resolve every registered module's `ability` declarations to their
/// content-addressed identities, keyed by their [`Fqn`](crate::fqn::Fqn).
///
/// The build hands this to the compiler as its foreign-ability channel:
/// performs and handler arms that the resolve pass canonicalized to a
/// foreign module need the interface identity and method order, which no
/// name→hash table carries. Identity is deterministic (the interface
/// hash), so recomputing here matches the declaring module's own
/// registration exactly. Cross-module dependency resolution failures are
/// ignored — they don't affect identity and are reported when the
/// declaring module checks.
#[must_use]
pub fn resolve_registry_abilities(
    registry: &ModuleRegistry,
) -> Vec<(crate::fqn::Fqn, Arc<crate::ability_resolver::DynAbility>)> {
    // Same context as `resolve_ability_declarations`, so the two paths
    // compute identical ability ids.
    let mut infer = ability_id_infer(registry);
    let mut discarded = Vec::new();
    let mut out = Vec::new();
    let mut modules: Vec<_> = registry
        .all_modules()
        .map(|info| (info.path.clone(), Arc::clone(&info.module)))
        .collect();
    modules.sort_by_key(|(path, _)| {
        // The declaration module (`core::system`) first, then path order.
        let key = path.to_string();
        (key != "core::system", key)
    });
    for (path, module) in modules {
        let namespace = registry.module_id(&path);
        for item in &module.items {
            if let crate::ast::ItemKind::Ability(def) = &item.kind {
                let dyn_ab = resolve_ability_def(&mut infer, def, &mut discarded);
                infer
                    .ability_resolver
                    .register_dynamic_in_namespace(&namespace, dyn_ab);
                if let Some(ability) = infer.ability_resolver.get_namespaced(&namespace, &def.name)
                {
                    let fqn = crate::fqn::Fqn::new(namespace.clone(), vec![Arc::clone(&def.name)]);
                    out.push((fqn, Arc::clone(ability)));
                }
            }
        }
    }
    out
}
