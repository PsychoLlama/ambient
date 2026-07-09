//! Ability resolver for looking up module-declared abilities.
//!
//! The `AbilityResolver` aggregates the abilities in scope for a compile —
//! local declarations and namespaced dynamics (ability preludes such as
//! `core::system`, and the prelude-injected `core::exception`) — and provides
//! lookup methods for the type checker and compiler. Every ability is
//! content-addressed and resolved from an in-language `ability` declaration;
//! there are no engine builtins.

use crate::fqn::ModuleId;
use crate::types::Type;
use ambient_core::{AbilityId, SignatureHash};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// One method of a module-declared ability, with resolved types.
///
/// `params`/`ret` are the declared types with type parameters substituted
/// for quantified type variables (listed in `quantified`); call sites
/// instantiate fresh variables for them. `signature` is the canonical
/// rendering's hash — one of the inputs to the method's
/// [`MethodKey`](ambient_core::MethodKey); the other (the default
/// implementation's content hash) exists only after compilation, so the
/// key itself is derived by the compiler and VM, never stored here.
#[derive(Debug, Clone)]
pub struct DynMethod {
    /// Method name as written in source.
    pub name: Arc<str>,
    /// Declared parameter names, parallel to `params` (for tooling:
    /// completion snippets, signature help).
    pub param_names: Vec<Arc<str>>,
    /// Declared parameter types.
    pub params: Vec<Type>,
    /// Declared return type.
    pub ret: Type,
    /// Type variable IDs standing in for the method's type parameters.
    pub quantified: Vec<crate::types::TypeVarId>,
    /// Hash of the canonical signature rendering.
    pub signature: SignatureHash,
    /// Whether the method carries a default implementation. `false` only
    /// for the abstract `Exception::throw` carve-out.
    pub has_impl: bool,
}

/// One ability method's full signature. For tooling: completions, hover,
/// signature help.
#[derive(Debug, Clone)]
pub struct MethodSignatureInfo {
    /// Method name.
    pub name: Arc<str>,
    /// Declared parameter names.
    pub param_names: Vec<Arc<str>>,
    /// Parameter types.
    pub params: Vec<Type>,
    /// Return type.
    pub ret: Type,
}

/// A module-declared ability: interface data resolved from source.
///
/// Plain data built by the type checker from `ability` declarations; the
/// identity is derived from the declaration's `unique(<uuid>)` prefix, so
/// it is stable under renames and moves and never collides with another
/// declaration's.
#[derive(Debug, Clone)]
pub struct DynAbility {
    /// The uuid-derived identity ([`AbilityId::from_uuid`]).
    pub id: AbilityId,
    /// The declaration uuid the identity derives from.
    pub uuid: Uuid,
    /// Ability name as written in source.
    pub name: Arc<str>,
    /// Methods in declaration order.
    pub methods: Vec<DynMethod>,
    /// Resolved identities of `with`-dependencies.
    pub dependencies: Vec<AbilityId>,
}

impl DynAbility {
    /// Look up a method by name.
    #[must_use]
    pub fn method(&self, name: &str) -> Option<&DynMethod> {
        self.methods.iter().find(|m| m.name.as_ref() == name)
    }
}

/// Resolves ability lookups from module-declared abilities.
///
/// This is used by the type checker and compiler to look up ability and method
/// information without hard-coding the ability definitions.
///
/// Every ability is a module-declared dynamic — including the language-level
/// `Exception`, which lives in `core::exception` and reaches every module
/// through the prelude. Two populations live here: local dynamics (declared
/// in the module being checked) and namespaced dynamics (declared elsewhere,
/// keyed by their declaring module — ability preludes such as `core::system`,
/// and `core::exception`). Source references resolve through
/// [`AbilityResolver::resolve_ref`], which enforces the namespace policy: a
/// reference resolves under the namespace its declaring module gives it, and
/// local declarations resolve bare and shadow same-named namespaced ones. The
/// remaining name lookups are low-level (tooling/rendering) where
/// qualification may be absent.
pub struct AbilityResolver {
    /// Module-declared abilities by name.
    dynamic_by_name: HashMap<Arc<str>, Arc<DynAbility>>,

    /// Module-declared abilities by identity.
    ///
    /// Covers both local and namespaced dynamics, so identity-keyed
    /// lookups (`id_to_name`, method signatures, handler-literal
    /// inference) treat them uniformly.
    dynamic_by_id: HashMap<AbilityId, Arc<DynAbility>>,

    /// Namespaced dynamic abilities: ([`ModuleId`], name) → ability.
    ///
    /// A namespace is the ability's declaring module: the `core::system`
    /// declaration module, or any module in the build (`utils`,
    /// `deep::nested::effects`). The resolve pass canonicalizes every
    /// qualified or imported ability reference to its declaring module's
    /// [`Fqn`](crate::fqn::Fqn), whose [`ModuleId`] keys this table — so
    /// performs (`core::system::Stdio::out!`), `with` clauses, effect-row
    /// annotations, handler arms, and sandbox clauses all resolve here (see
    /// [`AbilityResolver::resolve_ref`]).
    namespaced_by_name: HashMap<(ModuleId, Arc<str>), Arc<DynAbility>>,
}

/// Why a namespace-aware ability reference failed to resolve.
///
/// Produced by [`AbilityResolver::resolve_ref`], the policy-enforcing
/// entry point every source position that names an ability goes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbilityRefError {
    /// The name belongs to a namespaced dynamic but was written bare (or
    /// under the wrong namespace). The caller should tell the user to
    /// qualify it with this namespace.
    RequiresNamespace {
        /// The namespace the ability was registered under.
        namespace: Arc<str>,
    },
    /// No ability answers to this reference.
    Unknown,
}

impl AbilityResolver {
    /// Create a new empty ability resolver.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dynamic_by_name: HashMap::new(),
            dynamic_by_id: HashMap::new(),
            namespaced_by_name: HashMap::new(),
        }
    }

    /// Register a module-declared ability.
    pub fn register_dynamic(&mut self, ability: DynAbility) {
        let ability = Arc::new(ability);
        self.dynamic_by_name
            .insert(Arc::clone(&ability.name), Arc::clone(&ability));
        self.dynamic_by_id.insert(ability.id, ability);
    }

    /// Register a dynamic ability under a namespace ([`ModuleId`]).
    ///
    /// Namespaced abilities are referenced with their namespace prefix
    /// (`<namespace>::<Ability>`) in every source position; they do not
    /// shadow bare-name lookups of local declarations.
    pub fn register_dynamic_in_namespace(&mut self, namespace: &ModuleId, ability: DynAbility) {
        let ability = Arc::new(ability);
        self.namespaced_by_name.insert(
            (namespace.clone(), Arc::clone(&ability.name)),
            Arc::clone(&ability),
        );
        self.dynamic_by_id.insert(ability.id, ability);
    }

    /// Look up a module-declared ability by name.
    #[must_use]
    pub fn get_dynamic(&self, name: &str) -> Option<&Arc<DynAbility>> {
        self.dynamic_by_name.get(name)
    }

    /// Look up a namespaced dynamic ability by namespace and name.
    #[must_use]
    pub fn get_namespaced(&self, namespace: &ModuleId, name: &str) -> Option<&Arc<DynAbility>> {
        self.namespaced_by_name
            .get(&(namespace.clone(), Arc::from(name)))
    }

    /// A namespace a dynamic ability of this name was registered under, if
    /// any (used to suggest the qualifier for a bare misspelling).
    #[must_use]
    pub fn dynamic_namespace_of(&self, name: &str) -> Option<&ModuleId> {
        self.namespaced_by_name
            .keys()
            .find(|(_, n)| n.as_ref() == name)
            .map(|(ns, _)| ns)
    }

    /// Look up a module-declared ability by identity.
    #[must_use]
    pub fn get_dynamic_by_id(&self, id: AbilityId) -> Option<&Arc<DynAbility>> {
        self.dynamic_by_id.get(&id)
    }

    /// Resolve an ability reference as written in source, enforcing the
    /// namespace policy. This is the single entry point for every source
    /// position that names an ability: performs, `with` clauses,
    /// effect-row annotations, handler arms, and sandbox clauses.
    ///
    /// The policy:
    /// - A bare name resolves to a local (module-declared) dynamic. A bare
    ///   name that belongs to a namespaced dynamic is an error: namespaced
    ///   abilities must be written with their prefix
    ///   (`core::system::Stdio`). (A prelude-injected ability such as
    ///   `Exception` is spelled bare but carries its declaring module's
    ///   namespace after the resolve pass, so it takes the `Some` arm.)
    /// - A qualified name (`core::system::Stdio`, `core::exception::Exception`)
    ///   resolves only to the dynamic registered under exactly that
    ///   namespace. Locals may not be spelled with a namespace.
    /// - A local dynamic that shadows a namespaced name keeps working
    ///   bare; the namespaced one stays reachable via its prefix.
    ///
    /// # Errors
    ///
    /// [`AbilityRefError::RequiresNamespace`] when a namespaced dynamic
    /// was named bare or under the wrong namespace;
    /// [`AbilityRefError::Unknown`] otherwise.
    pub fn resolve_ref(
        &self,
        namespace: Option<&ModuleId>,
        name: &str,
    ) -> Result<AbilityId, AbilityRefError> {
        match namespace {
            None => {
                if let Some(dynamic) = self.dynamic_by_name.get(name) {
                    return Ok(dynamic.id);
                }
                if let Some(namespace) = self.dynamic_namespace_of(name) {
                    return Err(AbilityRefError::RequiresNamespace {
                        namespace: Arc::from(namespace.to_string()),
                    });
                }
                Err(AbilityRefError::Unknown)
            }
            Some(namespace) => {
                if let Some(ability) = self.get_namespaced(namespace, name) {
                    return Ok(ability.id);
                }
                // The right ability under the wrong qualifier still points
                // the user at the namespace that would work.
                if let Some(namespace) = self.dynamic_namespace_of(name) {
                    return Err(AbilityRefError::RequiresNamespace {
                        namespace: Arc::from(namespace.to_string()),
                    });
                }
                Err(AbilityRefError::Unknown)
            }
        }
    }

    /// Convert an ability name to its ID.
    ///
    /// This is a low-level bare-name lookup (local dynamics, then
    /// namespaced dynamics) for tooling and rendering, where qualification
    /// may be absent. Source positions that name abilities must go through
    /// [`Self::resolve_ref`], which enforces the namespace policy.
    #[must_use]
    pub fn name_to_id(&self, name: &str) -> Option<AbilityId> {
        if let Some(dynamic) = self.dynamic_by_name.get(name) {
            return Some(dynamic.id);
        }
        self.namespaced_dynamic_by_bare_name(name)
            .map(|dynamic| dynamic.id)
    }

    /// The first namespaced dynamic answering to a bare name, if any
    /// (tooling/rendering fallback where qualification may be absent).
    fn namespaced_dynamic_by_bare_name(&self, name: &str) -> Option<&Arc<DynAbility>> {
        self.namespaced_by_name
            .iter()
            .find(|((_, n), _)| n.as_ref() == name)
            .map(|(_, ability)| ability)
    }

    /// Convert an ability ID to its name.
    #[must_use]
    pub fn id_to_name(&self, id: AbilityId) -> Option<&str> {
        self.dynamic_by_id
            .get(&id)
            .map(|dynamic| dynamic.name.as_ref())
    }

    /// Get all method signatures for an ability. For tooling: completions,
    /// hover, signature help.
    #[must_use]
    pub fn method_signatures(&self, ability_id: AbilityId) -> Vec<MethodSignatureInfo> {
        let Some(dynamic) = self.dynamic_by_id.get(&ability_id) else {
            return vec![];
        };
        dynamic
            .methods
            .iter()
            .map(|m| MethodSignatureInfo {
                name: Arc::clone(&m.name),
                param_names: m.param_names.clone(),
                params: m.params.clone(),
                ret: m.ret.clone(),
            })
            .collect()
    }

    /// Every ability name spelled the way source must reference it: local
    /// dynamics bare, namespaced dynamics with their prefix
    /// (`core::system::Stdio`). Sorted and deduplicated. Suitable for
    /// completions in `with` clauses and handler arms.
    #[must_use]
    pub fn ability_names(&self) -> Vec<Arc<str>> {
        let mut names: Vec<Arc<str>> = self
            .dynamic_by_name
            .keys()
            .cloned()
            .chain(
                self.namespaced_by_name
                    .keys()
                    .map(|(namespace, name)| Arc::from(format!("{namespace}::{name}"))),
            )
            // `namespace` renders via `ModuleId`'s `Display`.
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Bare names of the dynamic abilities registered under a namespace,
    /// sorted. For tooling: completing `core::system::` offers these.
    #[must_use]
    pub fn namespace_ability_names(&self, namespace: &ModuleId) -> Vec<Arc<str>> {
        let mut names: Vec<Arc<str>> = self
            .namespaced_by_name
            .keys()
            .filter(|(ns, _)| ns == namespace)
            .map(|(_, name)| Arc::clone(name))
            .collect();
        names.sort_unstable();
        names
    }

    /// Get the declared return type for a method.
    #[must_use]
    pub fn get_method_return_type(&self, ability_name: &str, method_name: &str) -> Option<Type> {
        if let Some(dynamic) = self.dynamic_by_name.get(ability_name) {
            return dynamic.method(method_name).map(|m| m.ret.clone());
        }
        self.namespaced_dynamic_by_bare_name(ability_name)
            .and_then(|dynamic| dynamic.method(method_name))
            .map(|m| m.ret.clone())
    }

    /// Check if a method exists for an ability.
    #[must_use]
    pub fn has_method(&self, ability_name: &str, method_name: &str) -> bool {
        if let Some(dynamic) = self.dynamic_by_name.get(ability_name) {
            return dynamic.method(method_name).is_some();
        }
        self.namespaced_dynamic_by_bare_name(ability_name)
            .is_some_and(|dynamic| dynamic.method(method_name).is_some())
    }
}

impl Default for AbilityResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Canonical type rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Renders engine [`Type`]s into the canonical string grammar that ability
/// interface hashing uses (see `ambient_core::canonical`).
///
/// Primitives match `CanonicalTypeFactory` exactly ("unit", "number",
/// "list<...>", ...), so a builtin descriptor and an in-language
/// declaration of the same monomorphic interface hash identically. Type
/// variables (and each `Hole` occurrence) are numbered by first
/// appearance within one renderer instance; use one renderer per method
/// signature so numbering is signature-local.
///
/// Uuid-carrying nominal types (declared enums, opaque-generic containers,
/// `extern` structs) render by their **uuid** (`named:<uuid>`,
/// `nominal:<uuid>:...`): the uuid is the type's stable identity, so
/// renaming the type never moves a method's hash. Primitives stay bare
/// words and structural types (tuples, records, functions) render by
/// shape. A name that resolved to no uuid — a cross-module nominal like
/// `Duration` that stays an unresolved `Named` because ability signatures
/// resolve before the module's alias table is populated, and
/// type-parameter references — falls back to `named:<name>`. That fallback
/// is byte-stable (the same name renders the same way on every path) but
/// is *not* rename-stable, the documented limit of the uuid scheme.
#[derive(Debug, Default)]
pub struct CanonicalTypeRenderer {
    vars: HashMap<crate::types::TypeVarId, u32>,
    next_var: u32,
}

impl CanonicalTypeRenderer {
    /// Create a renderer with variable numbering starting at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn fresh(&mut self) -> String {
        let id = self.next_var;
        self.next_var += 1;
        format!("var{id}")
    }

    /// Render a type into its canonical string form.
    pub fn render(&mut self, ty: &Type) -> String {
        // Primitives are `extern` `Nominal` types carrying a reserved uuid, but
        // the canonical interface grammar renders them as bare lowercase words
        // (`string`, `number`, ...) to match `TypeFactory` (see
        // `ambient-core::canonical`), keeping ability interface identities
        // byte-stable across the upgrade.
        if let Some(prim) = ty.as_primitive() {
            return match prim {
                crate::types::Primitive::Bool => "bool",
                crate::types::Primitive::Number => "number",
                crate::types::Primitive::String => "string",
                crate::types::Primitive::Binary => "binary",
            }
            .to_string();
        }
        match ty {
            Type::Unit => "unit".to_string(),
            Type::Never => "never".to_string(),
            // Each hole is an independent unknown, matching the factory's
            // per-call freshness.
            Type::Hole | Type::Error => self.fresh(),
            Type::Var(id) => {
                if let Some(&n) = self.vars.get(id) {
                    format!("var{n}")
                } else {
                    let n = self.next_var;
                    self.next_var += 1;
                    self.vars.insert(*id, n);
                    format!("var{n}")
                }
            }
            Type::Named(named) if named.name.as_ref() == "List" && named.args.len() == 1 => {
                format!("list<{}>", self.render(&named.args[0]))
            }
            Type::Named(named) => {
                // A resolved nominal head renders by its uuid so renaming the
                // type is hash-stable; an unresolved name (cross-module
                // `Duration`, type-parameter references) falls back to the
                // name — byte-stable across paths, but not rename-stable.
                let head = match named.uuid {
                    Some(uuid) => format!("named:{uuid}"),
                    None => format!("named:{}", named.name),
                };
                if named.args.is_empty() {
                    head
                } else {
                    let args: Vec<String> = named.args.iter().map(|a| self.render(a)).collect();
                    format!("{head}<{}>", args.join(", "))
                }
            }
            Type::Tuple(elems) => {
                let elems: Vec<String> = elems.iter().map(|e| self.render(e)).collect();
                format!("({})", elems.join(", "))
            }
            Type::Record(record) => {
                let fields: Vec<String> = record
                    .fields
                    .iter()
                    .map(|(name, ty)| format!("{name}: {}", self.render(ty)))
                    .collect();
                format!("{{{}}}", fields.join(", "))
            }
            Type::Function(func) => {
                let params: Vec<String> = func.params.iter().map(|p| self.render(p)).collect();
                let ret = self.render(&func.ret);
                let abilities = self.render_ability_set(&func.abilities);
                if abilities.is_empty() {
                    format!("fn({}) -> {ret}", params.join(", "))
                } else {
                    format!("fn({}) -> {ret} with {abilities}", params.join(", "))
                }
            }
            Type::Forall(forall) => {
                // Quantified variables are numbered like any other on first
                // occurrence inside the body.
                format!("forall({})", self.render(&forall.body))
            }
            Type::Nominal(nominal) => {
                // A nominal always carries a uuid — its stable identity — so
                // render by it (the `name` is a human-facing label only,
                // and renaming it must not move the hash).
                let inner = self.render(&nominal.inner);
                format!("nominal:{}:{inner}", nominal.uuid)
            }
            Type::AbilityValue(av) => {
                let result = self.render(&av.result);
                let abilities = self.render_ability_set(&av.ability);
                format!("ability<{result}, {{{abilities}}}>")
            }
            // A rigid type parameter renders byte-identically to the
            // unresolved `Named{args:[]}` it replaced (`named:T`), so ability
            // interface hashes are invariant even if a `Param` were to reach a
            // renderer. It shouldn't: signature schemes substitute type
            // parameters to `Var` before rendering (see `resolve_ability_def`),
            // so this arm is a defensive tripwire, not a live path.
            Type::Param(name) => {
                debug_assert!(false, "Type::Param reached CanonicalTypeRenderer: {name}");
                format!("named:{name}")
            }
            Type::Handler(handler) => format!("handler<{}>", handler.ability.to_hex()),
            // `resolve_ability_def` resolves signatures before rendering, so
            // the unresolved surface form never reaches interface hashing —
            // a tripwire, not a live path.
            Type::HandlerAnnotation(h) => {
                debug_assert!(
                    false,
                    "HandlerAnnotation reached CanonicalTypeRenderer: {h:?}"
                );
                format!("handler<?{}>", h.ability)
            }
        }
    }

    #[allow(clippy::unused_self)] // symmetry with render(); may number ability vars later
    fn render_ability_set(&mut self, set: &crate::types::AbilitySet) -> String {
        use crate::types::AbilitySet;
        match set {
            AbilitySet::Empty => String::new(),
            AbilitySet::Concrete(ids) => {
                let mut ids: Vec<String> = ids.iter().map(AbilityId::to_hex).collect();
                ids.sort_unstable();
                ids.join(", ")
            }
            AbilitySet::Var(_) => "e".to_string(),
            AbilitySet::Row { concrete, tail: _ } => {
                let mut ids: Vec<String> = concrete.iter().map(AbilityId::to_hex).collect();
                ids.sort_unstable();
                ids.push("e".to_string());
                ids.join(", ")
            }
            // Unresolved names never survive type checking.
            AbilitySet::Unresolved(names) => {
                let mut names: Vec<String> = names.iter().map(|n| format!("?{n}")).collect();
                names.sort_unstable();
                names.join(", ")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dyn_ability(name: &str, byte: u8) -> DynAbility {
        DynAbility {
            id: AbilityId::from_bytes([byte; 32]),
            uuid: Uuid::from_u128(u128::from(byte)),
            name: Arc::from(name),
            methods: vec![DynMethod {
                name: Arc::from("go"),
                param_names: vec![],
                params: vec![Type::string()],
                ret: Type::Unit,
                quantified: vec![],
                signature: ambient_core::SignatureHash::new(&["string"], "unit"),
                has_impl: true,
            }],
            dependencies: vec![],
        }
    }

    #[test]
    fn namespaced_dynamics_require_their_namespace() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic_in_namespace(&ModuleId::core_system(), dyn_ability("Printer", 7));

        // Qualified lookup finds it; the wrong namespace does not.
        assert!(
            resolver
                .get_namespaced(&ModuleId::core_system(), "Printer")
                .is_some()
        );
        assert!(
            resolver
                .get_namespaced(&ModuleId::builtin(&["other"]), "Printer")
                .is_none()
        );
        assert_eq!(
            resolver.dynamic_namespace_of("Printer"),
            Some(&ModuleId::core_system())
        );

        // Source references resolve only with the namespace prefix.
        assert_eq!(
            resolver.resolve_ref(Some(&ModuleId::core_system()), "Printer"),
            Ok(AbilityId::from_bytes([7; 32]))
        );
        assert_eq!(
            resolver.resolve_ref(None, "Printer"),
            Err(AbilityRefError::RequiresNamespace {
                namespace: Arc::from("core::system"),
            })
        );
        assert_eq!(
            resolver.resolve_ref(Some(&ModuleId::builtin(&["other"])), "Printer"),
            Err(AbilityRefError::RequiresNamespace {
                namespace: Arc::from("core::system"),
            })
        );

        // Low-level bare lookups keep working for tooling/rendering.
        assert_eq!(
            resolver.name_to_id("Printer"),
            Some(AbilityId::from_bytes([7; 32]))
        );
        assert!(resolver.has_method("Printer", "go"));

        // Identity-keyed lookups treat namespaced dynamics uniformly.
        assert_eq!(
            resolver.id_to_name(AbilityId::from_bytes([7; 32])),
            Some("Printer")
        );
        assert!(
            resolver
                .get_dynamic_by_id(AbilityId::from_bytes([7; 32]))
                .is_some()
        );

        // But it is not a local dynamic.
        assert!(resolver.get_dynamic("Printer").is_none());
    }

    #[test]
    fn resolve_ref_policy_for_locals() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic(dyn_ability("Printer", 9));

        // A local declaration resolves bare.
        assert_eq!(
            resolver.resolve_ref(None, "Printer"),
            Ok(AbilityId::from_bytes([9; 32]))
        );

        // A local may not be spelled with a namespace.
        assert_eq!(
            resolver.resolve_ref(Some(&ModuleId::core_system()), "Printer"),
            Err(AbilityRefError::Unknown)
        );

        // Unknown names are unknown, qualified or not.
        assert_eq!(
            resolver.resolve_ref(None, "Nope"),
            Err(AbilityRefError::Unknown)
        );
        assert_eq!(
            resolver.resolve_ref(Some(&ModuleId::builtin(&["system", "extra"])), "Printer"),
            Err(AbilityRefError::Unknown)
        );
    }

    #[test]
    fn local_dynamics_shadow_namespaced_in_bare_lookups() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic_in_namespace(&ModuleId::core_system(), dyn_ability("Printer", 7));
        resolver.register_dynamic(dyn_ability("Printer", 9));

        // The bare reference means the local declaration.
        assert_eq!(
            resolver.resolve_ref(None, "Printer"),
            Ok(AbilityId::from_bytes([9; 32]))
        );
        assert_eq!(
            resolver.name_to_id("Printer"),
            Some(AbilityId::from_bytes([9; 32]))
        );
        // The namespaced one remains reachable via qualification.
        assert_eq!(
            resolver.resolve_ref(Some(&ModuleId::core_system()), "Printer"),
            Ok(AbilityId::from_bytes([7; 32]))
        );
        assert_eq!(
            resolver
                .get_namespaced(&ModuleId::core_system(), "Printer")
                .map(|a| a.id),
            Some(AbilityId::from_bytes([7; 32]))
        );
    }

    #[test]
    fn ability_names_render_namespaced_qualified() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic_in_namespace(&ModuleId::core_system(), dyn_ability("Printer", 7));
        resolver.register_dynamic(dyn_ability("Local", 9));

        let names = resolver.ability_names();
        assert!(names.iter().any(|n| n.as_ref() == "core::system::Printer"));
        assert!(names.iter().any(|n| n.as_ref() == "Local"));
        assert!(!names.iter().any(|n| n.as_ref() == "Printer"));
    }
}
