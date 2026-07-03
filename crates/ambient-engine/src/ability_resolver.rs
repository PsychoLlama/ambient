//! Ability resolver for looking up abilities from registered providers.
//!
//! The `AbilityResolver` aggregates abilities from multiple providers (core, platform,
//! and any user-defined providers) and provides lookup methods for the type checker
//! and compiler.

use crate::types::Type;
use ambient_core::{
    AbilityDescriptor, AbilityId, AbilityProvider, MethodId, RawMethod, TypeFactory,
    hash_interface_raw,
};
use std::collections::HashMap;
use std::sync::Arc;

/// One method of a module-declared ability, with resolved types.
///
/// The method's ID is its declaration index. `params`/`ret` are the
/// declared types with type parameters substituted for quantified type
/// variables (listed in `quantified`); call sites instantiate fresh
/// variables for them.
#[derive(Debug, Clone)]
pub struct DynMethod {
    /// Declaration index within the ability.
    pub id: MethodId,
    /// Method name as written in source.
    pub name: Arc<str>,
    /// Declared parameter types.
    pub params: Vec<Type>,
    /// Declared return type.
    pub ret: Type,
    /// Type variable IDs standing in for the method's type parameters.
    pub quantified: Vec<crate::types::TypeVarId>,
}

/// A module-declared ability: interface data resolved from source.
///
/// Unlike builtin [`AbilityDescriptor`]s (compile-time constants with
/// factory-function signatures), these are plain data built by the type
/// checker from `ability` declarations. Their identity is the same
/// canonical interface hash builtins use.
#[derive(Debug, Clone)]
pub struct DynAbility {
    /// Content-addressed identity of the interface.
    pub id: AbilityId,
    /// Ability name as written in source.
    pub name: Arc<str>,
    /// Methods in declaration order (method ID = declaration index).
    pub methods: Vec<DynMethod>,
    /// Resolved identities of `with`-dependencies.
    pub dependencies: Vec<AbilityId>,
}

impl DynAbility {
    /// Compute the interface identity from canonical signature renderings.
    ///
    /// `canonical_methods` must contain, per method, the canonical string
    /// forms of its parameter and return types (see
    /// [`canonical_type_string`]).
    #[must_use]
    pub fn hash_from_canonical(
        name: &str,
        canonical_methods: &[(Arc<str>, Vec<String>, String)],
    ) -> AbilityId {
        #[allow(clippy::cast_possible_truncation)]
        let raw: Vec<RawMethod> = canonical_methods
            .iter()
            .enumerate()
            .map(|(idx, (name, params, ret))| RawMethod {
                id: idx as u16,
                name: name.to_string(),
                params: params.clone(),
                ret: ret.clone(),
            })
            .collect();
        hash_interface_raw(name, &raw)
    }

    /// Look up a method by name.
    #[must_use]
    pub fn method(&self, name: &str) -> Option<&DynMethod> {
        self.methods.iter().find(|m| m.name.as_ref() == name)
    }
}

/// A plain-data view of a resolved ability interface: its
/// content-addressed identity plus method-name → method-id mapping.
///
/// Unlike [`DynAbility`] this is `Send + Sync` (no types), so host
/// binding code — including capability-grant closures that outlive the
/// current thread — can carry it around freely.
#[derive(Debug, Clone)]
pub struct AbilityInterface {
    /// Content-addressed identity of the interface.
    pub id: AbilityId,
    methods: Vec<(Arc<str>, MethodId)>,
}

impl AbilityInterface {
    /// Method ID for a method name.
    #[must_use]
    pub fn method_id(&self, name: &str) -> Option<MethodId> {
        self.methods
            .iter()
            .find(|(n, _)| n.as_ref() == name)
            .map(|(_, id)| *id)
    }
}

impl From<&DynAbility> for AbilityInterface {
    fn from(ability: &DynAbility) -> Self {
        Self {
            id: ability.id,
            methods: ability
                .methods
                .iter()
                .map(|m| (Arc::clone(&m.name), m.id))
                .collect(),
        }
    }
}

/// Resolves ability lookups from registered providers.
///
/// This is used by the type checker and compiler to look up ability and method
/// information without hard-coding the ability definitions.
///
/// Two populations live here: builtin descriptors (registered from
/// providers/config) and dynamic, module-declared abilities. Name lookups
/// prefer dynamic abilities, so a module declaration shadows a builtin of
/// the same name (except the `platform.`-namespaced ones, which the type
/// checker guards separately).
pub struct AbilityResolver {
    /// Map from ability name to descriptor.
    by_name: HashMap<Arc<str>, AbilityDescriptor<Type>>,

    /// Map from ability ID to descriptor.
    by_id: HashMap<AbilityId, AbilityDescriptor<Type>>,

    /// Module-declared abilities by name.
    dynamic_by_name: HashMap<Arc<str>, Arc<DynAbility>>,

    /// Module-declared abilities by identity.
    ///
    /// Covers both local and namespaced dynamics, so identity-keyed
    /// lookups (`id_to_name`, method signatures, handler-literal
    /// inference) treat them uniformly.
    dynamic_by_id: HashMap<AbilityId, Arc<DynAbility>>,

    /// Namespaced dynamic abilities: name → (namespace, ability).
    ///
    /// These come from ability preludes (declaration modules an embedder
    /// registers, e.g. the `platform` module). Unlike local dynamics they
    /// must be performed with their namespace prefix
    /// (`platform.Console.print!`); bare names still resolve in effect
    /// rows and handler arms, where no qualification exists today.
    namespaced_by_name: HashMap<Arc<str>, (Arc<str>, Arc<DynAbility>)>,
}

impl AbilityResolver {
    /// Create a new empty ability resolver.
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            dynamic_by_name: HashMap::new(),
            dynamic_by_id: HashMap::new(),
            namespaced_by_name: HashMap::new(),
        }
    }

    /// Register a module-declared ability.
    // `Type` uses `Rc` internally (never crosses threads during checking),
    // but `Arc` matches the rest of the engine's shared-ownership idiom.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn register_dynamic(&mut self, ability: DynAbility) {
        let ability = Arc::new(ability);
        self.dynamic_by_name
            .insert(Arc::clone(&ability.name), Arc::clone(&ability));
        self.dynamic_by_id.insert(ability.id, ability);
    }

    /// Register a dynamic ability under a namespace.
    ///
    /// Namespaced abilities are performed with their namespace prefix
    /// (`<namespace>.<Ability>.<method>!`); they do not shadow bare-name
    /// lookups of local declarations.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn register_dynamic_in_namespace(&mut self, namespace: &str, ability: DynAbility) {
        let ability = Arc::new(ability);
        self.namespaced_by_name.insert(
            Arc::clone(&ability.name),
            (Arc::from(namespace), Arc::clone(&ability)),
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
    pub fn get_namespaced(&self, namespace: &str, name: &str) -> Option<&Arc<DynAbility>> {
        self.namespaced_by_name
            .get(name)
            .filter(|(ns, _)| ns.as_ref() == namespace)
            .map(|(_, ability)| ability)
    }

    /// The namespace a dynamic ability was registered under, if any.
    #[must_use]
    pub fn dynamic_namespace_of(&self, name: &str) -> Option<&Arc<str>> {
        self.namespaced_by_name.get(name).map(|(ns, _)| ns)
    }

    /// Look up a module-declared ability by identity.
    #[must_use]
    pub fn get_dynamic_by_id(&self, id: AbilityId) -> Option<&Arc<DynAbility>> {
        self.dynamic_by_id.get(&id)
    }

    /// Register abilities from a provider.
    pub fn register<P: AbilityProvider<Type>>(&mut self, provider: &P) {
        for ability in provider.abilities() {
            self.by_name
                .insert(Arc::from(ability.name), ability.clone());
            self.by_id.insert(ability.id, ability.clone());
        }
    }

    /// Look up an ability by name.
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<&AbilityDescriptor<Type>> {
        self.by_name.get(name)
    }

    /// Look up an ability by ID.
    #[must_use]
    pub fn get_by_id(&self, id: AbilityId) -> Option<&AbilityDescriptor<Type>> {
        self.by_id.get(&id)
    }

    /// Convert an ability name to its ID.
    ///
    /// Bare names resolve through local dynamics, then namespaced
    /// dynamics, then builtin descriptors: effect rows and handler arms
    /// name abilities without qualification, so prelude abilities must be
    /// reachable here, while local declarations still shadow them.
    #[must_use]
    pub fn name_to_id(&self, name: &str) -> Option<AbilityId> {
        if let Some(dynamic) = self.dynamic_by_name.get(name) {
            return Some(dynamic.id);
        }
        if let Some((_, dynamic)) = self.namespaced_by_name.get(name) {
            return Some(dynamic.id);
        }
        self.by_name.get(name).map(|a| a.id)
    }

    /// Convert an ability ID to its name.
    #[must_use]
    pub fn id_to_name(&self, id: AbilityId) -> Option<&str> {
        if let Some(dynamic) = self.dynamic_by_id.get(&id) {
            return Some(dynamic.name.as_ref());
        }
        self.by_id.get(&id).map(|a| a.name)
    }

    /// Look up a method by ability name and method name.
    #[must_use]
    pub fn get_method(
        &self,
        ability_name: &str,
        method_name: &str,
    ) -> Option<(AbilityId, MethodId)> {
        if let Some(dynamic) = self.dynamic_by_name.get(ability_name) {
            let method = dynamic.method(method_name)?;
            return Some((dynamic.id, method.id));
        }
        if let Some((_, dynamic)) = self.namespaced_by_name.get(ability_name) {
            let method = dynamic.method(method_name)?;
            return Some((dynamic.id, method.id));
        }
        let ability = self.by_name.get(ability_name)?;
        let method = ability.get_method(method_name)?;
        Some((ability.id, method.id))
    }

    /// Look up a method by ability ID and method name.
    #[must_use]
    pub fn get_method_by_ability_id(
        &self,
        ability_id: AbilityId,
        method_name: &str,
    ) -> Option<MethodId> {
        if let Some(dynamic) = self.dynamic_by_id.get(&ability_id) {
            return dynamic.method(method_name).map(|m| m.id);
        }
        let ability = self.by_id.get(&ability_id)?;
        let method = ability.get_method(method_name)?;
        Some(method.id)
    }

    /// Get all method signatures for an ability.
    ///
    /// Returns a list of tuples containing method name, parameter count, and return type.
    /// Method names are cloned to avoid lifetime issues.
    #[must_use]
    pub fn get_method_signatures(
        &self,
        ability_id: AbilityId,
        type_factory: &dyn TypeFactory<Type>,
    ) -> Vec<(String, usize, Type)> {
        if let Some(dynamic) = self.dynamic_by_id.get(&ability_id) {
            return dynamic
                .methods
                .iter()
                .map(|m| (m.name.to_string(), m.params.len(), m.ret.clone()))
                .collect();
        }

        let Some(ability) = self.by_id.get(&ability_id) else {
            return vec![];
        };

        ability
            .methods
            .iter()
            .map(|m| {
                let return_type = (m.signature.return_type)(type_factory);
                (m.name.to_string(), m.signature.param_count, return_type)
            })
            .collect()
    }

    /// Try to infer which ability a handler literal is for based on method names.
    ///
    /// Returns the ability ID if all methods belong to exactly one ability.
    #[must_use]
    pub fn infer_ability_from_methods(&self, method_names: &[Arc<str>]) -> Option<AbilityId> {
        if method_names.is_empty() {
            return None;
        }

        let mut matching_abilities = Vec::new();

        for ability in self.by_id.values() {
            let ability_methods: Vec<&str> = ability.methods.iter().map(|m| m.name).collect();

            let all_methods_match = method_names
                .iter()
                .all(|m| ability_methods.contains(&m.as_ref()));

            if all_methods_match {
                matching_abilities.push(ability.id);
            }
        }

        for ability in self.dynamic_by_id.values() {
            let all_methods_match = method_names
                .iter()
                .all(|m| ability.method(m.as_ref()).is_some());

            if all_methods_match {
                matching_abilities.push(ability.id);
            }
        }

        // A descriptor and a prelude declaration of the same interface
        // share an identity; count them once.
        matching_abilities.sort_unstable();
        matching_abilities.dedup();

        // Return only if exactly one ability matches
        if matching_abilities.len() == 1 {
            Some(matching_abilities[0])
        } else {
            None
        }
    }

    /// Get an iterator over all registered abilities.
    pub fn abilities(&self) -> impl Iterator<Item = &AbilityDescriptor<Type>> {
        self.by_id.values()
    }

    /// Get the return type for a method.
    ///
    /// Returns the return type constructed using the provided type factory.
    #[must_use]
    pub fn get_method_return_type(
        &self,
        ability_name: &str,
        method_name: &str,
        type_factory: &dyn TypeFactory<Type>,
    ) -> Option<Type> {
        if let Some(dynamic) = self.dynamic_by_name.get(ability_name) {
            return dynamic.method(method_name).map(|m| m.ret.clone());
        }
        if let Some((_, dynamic)) = self.namespaced_by_name.get(ability_name) {
            return dynamic.method(method_name).map(|m| m.ret.clone());
        }
        let ability = self.by_name.get(ability_name)?;
        let method = ability.get_method(method_name)?;
        Some((method.signature.return_type)(type_factory))
    }

    /// Check if a method exists for an ability.
    #[must_use]
    pub fn has_method(&self, ability_name: &str, method_name: &str) -> bool {
        if let Some(dynamic) = self.dynamic_by_name.get(ability_name) {
            return dynamic.method(method_name).is_some();
        }
        if let Some((_, dynamic)) = self.namespaced_by_name.get(ability_name) {
            return dynamic.method(method_name).is_some();
        }
        self.by_name
            .get(ability_name)
            .is_some_and(|a| a.get_method(method_name).is_some())
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
/// Nominal types render by name only: their UUIDs are freshly generated
/// per compilation and would break hash determinism.
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
        use crate::types::TypeVar;
        match ty {
            Type::Unit => "unit".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Number => "number".to_string(),
            Type::String => "string".to_string(),
            Type::Bytes => "bytes".to_string(),
            Type::Never => "never".to_string(),
            // Each hole is an independent unknown, matching the factory's
            // per-call freshness.
            Type::Hole | Type::Error => self.fresh(),
            Type::Var(TypeVar::Unbound(id)) => {
                if let Some(&n) = self.vars.get(id) {
                    format!("var{n}")
                } else {
                    let n = self.next_var;
                    self.next_var += 1;
                    self.vars.insert(*id, n);
                    format!("var{n}")
                }
            }
            Type::Var(TypeVar::Link(target)) => {
                let target = target.borrow().clone();
                self.render(&target)
            }
            Type::Named(named) if named.name.as_ref() == "List" && named.args.len() == 1 => {
                format!("list<{}>", self.render(&named.args[0]))
            }
            Type::Named(named) => {
                if named.args.is_empty() {
                    format!("named:{}", named.name)
                } else {
                    let args: Vec<String> = named.args.iter().map(|a| self.render(a)).collect();
                    format!("named:{}<{}>", named.name, args.join(", "))
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
                let inner = self.render(&nominal.inner);
                match &nominal.name {
                    Some(name) => format!("nominal:{name}:{inner}"),
                    None => format!("nominal:{inner}"),
                }
            }
            Type::AbilityValue(av) => {
                let result = self.render(&av.result);
                let abilities = self.render_ability_set(&av.ability);
                format!("ability<{result}, {{{abilities}}}>")
            }
            Type::Handler(handler) => format!("handler<{}>", handler.ability.to_hex()),
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

/// Type factory implementation for the engine's Type.
pub struct EngineTypeFactory;

impl TypeFactory<Type> for EngineTypeFactory {
    fn unit(&self) -> Type {
        Type::Unit
    }

    fn bool(&self) -> Type {
        Type::Bool
    }

    fn number(&self) -> Type {
        Type::Number
    }

    fn string(&self) -> Type {
        Type::String
    }

    fn bytes(&self) -> Type {
        Type::Bytes
    }

    fn never(&self) -> Type {
        Type::Never
    }

    fn type_var(&self) -> Type {
        // For type variables, we return a Hole which will be instantiated
        // during type inference. This is a simplification - in a full
        // implementation we'd track a counter.
        Type::Hole
    }

    fn list(&self, element: Type) -> Type {
        Type::named("List", vec![element])
    }
}

/// Create an `AbilityResolver` with the language-level core abilities
/// (Exception).
///
/// This is the engine's only builtin ability set. Platform abilities
/// (Console, `FileSystem`, Network, ...) are not engine builtins: embedders
/// resolve their declaration modules with
/// [`crate::infer::resolve_ability_declarations`] and register the
/// results as namespaced dynamics.
#[must_use]
pub fn core_abilities() -> AbilityResolver {
    let factory = EngineTypeFactory;
    let mut resolver = AbilityResolver::new();
    let core = ambient_core::CoreAbilities::new(&factory);
    resolver.register(&core);
    resolver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_abilities() {
        let resolver = core_abilities();

        // Exception is the engine's only builtin ability.
        assert!(resolver.get_by_name("Exception").is_some());

        // Platform abilities are not engine builtins.
        assert!(resolver.get_by_name("Console").is_none());
        assert!(resolver.get_by_name("FileSystem").is_none());
    }

    #[test]
    fn test_infer_ability_from_methods() {
        let mut resolver = core_abilities();
        resolver.register_dynamic_in_namespace("platform", dyn_ability("Printer", 7));

        // Methods that match the namespaced dynamic.
        let methods = vec![Arc::from("go")];
        let result = resolver.infer_ability_from_methods(&methods);
        assert_eq!(result, Some(AbilityId::from_bytes([7; 32])));

        // Methods that match Exception.
        let methods = vec![Arc::from("throw")];
        let result = resolver.infer_ability_from_methods(&methods);
        assert_eq!(result, Some(ambient_core::exception::ability_id()));
    }

    fn dyn_ability(name: &str, byte: u8) -> DynAbility {
        DynAbility {
            id: AbilityId::from_bytes([byte; 32]),
            name: Arc::from(name),
            methods: vec![DynMethod {
                id: 0,
                name: Arc::from("go"),
                params: vec![Type::String],
                ret: Type::Unit,
                quantified: vec![],
            }],
            dependencies: vec![],
        }
    }

    #[test]
    fn namespaced_dynamics_resolve_qualified_and_bare() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic_in_namespace("platform", dyn_ability("Printer", 7));

        // Qualified lookup finds it; the wrong namespace does not.
        assert!(resolver.get_namespaced("platform", "Printer").is_some());
        assert!(resolver.get_namespaced("other", "Printer").is_none());
        assert_eq!(
            resolver.dynamic_namespace_of("Printer").map(AsRef::as_ref),
            Some("platform")
        );

        // Bare names still resolve for effect rows and handler arms.
        assert_eq!(
            resolver.name_to_id("Printer"),
            Some(AbilityId::from_bytes([7; 32]))
        );
        assert_eq!(
            resolver.get_method("Printer", "go"),
            Some((AbilityId::from_bytes([7; 32]), 0))
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
    fn local_dynamics_shadow_namespaced_in_bare_lookups() {
        let mut resolver = AbilityResolver::new();
        resolver.register_dynamic_in_namespace("platform", dyn_ability("Printer", 7));
        resolver.register_dynamic(dyn_ability("Printer", 9));

        assert_eq!(
            resolver.name_to_id("Printer"),
            Some(AbilityId::from_bytes([9; 32]))
        );
        // The namespaced one remains reachable via qualification.
        assert_eq!(
            resolver.get_namespaced("platform", "Printer").map(|a| a.id),
            Some(AbilityId::from_bytes([7; 32]))
        );
    }
}
