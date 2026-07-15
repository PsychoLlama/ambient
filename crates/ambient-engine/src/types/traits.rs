//! The trait system: definitions, implementations, coherence, and the
//! impl-method dispatch symbols.
//!
//! Traits are **nominal**, exactly like enums, structs, and abilities: the
//! mandatory `unique(<uuid>)` prefix is the trait's identity. Every table
//! here keys off that uuid — never the name — so renaming a trait never
//! changes what a bound or an impl means, and two same-shaped traits never
//! unify. Names exist for display and for in-scope lookup only.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use super::Type;
use crate::fqn::Fqn;

// ─────────────────────────────────────────────────────────────────────────────
// Reserved trait identities
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical identity of the prelude `Add` trait (`core::traits::Add`).
///
/// The operator traits are ordinary declarations in `core::traits`, but the
/// engine's operator desugar (`a + b` → `a.add(b)`) must name *the* trait an
/// operator dispatches through, independent of what is lexically in scope —
/// a user trait named `Add` must never capture `+`. These reserved uuids are
/// that anchor, in the same `0xffff…` namespace as `Option`/`Result`
/// ([`super::OPTION_UUID`]); the source declarations in `core_lib/traits.ab`
/// claim them and are pinned by `validate_reserved_trait`, so the sources
/// and the engine can never drift and no other module can hijack one.
///
/// Discriminators `0x0010`–`0x001f` are reserved for traits; see
/// [`super::BOOL_UUID`] for how this namespace is allocated.
pub const TRAIT_ADD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0010);
/// Canonical identity of the prelude `Sub` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_SUB_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0011);
/// Canonical identity of the prelude `Mul` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_MUL_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0012);
/// Canonical identity of the prelude `Div` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_DIV_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0013);
/// Canonical identity of the prelude `Mod` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_MOD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0014);
/// Canonical identity of the prelude `Eq` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_EQ_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0015);
/// Canonical identity of the prelude `Ord` trait. See [`TRAIT_ADD_UUID`].
pub const TRAIT_ORD_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0016);
/// Canonical identity of the `core::traits::Default` trait (not in the
/// prelude — no operator desugars to it). See [`TRAIT_ADD_UUID`].
pub const TRAIT_DEFAULT_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0017);
/// Canonical identity of the `core::traits::Show` trait — the stringifier
/// (`fn show(self): String`) that bounds `Exception::throw<E: Show>`. Not an
/// operator (no desugar anchors on it), but reserved and prelude-exported so
/// `throw`'s bound is always in scope. See [`TRAIT_ADD_UUID`]; it claims the
/// slot after `Default` in the reserved trait block.
pub const TRAIT_SHOW_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0018);
/// Canonical identity of the prelude `From<T>` conversion trait
/// (`core::convert::From`): `fn from(value: T): Self`, the declaration side
/// of every value conversion. Reserved (not just a library trait) because
/// the bound solver's `Into` bridge anchors on the *pair* of uuids:
/// `S: Into<T>` is satisfiable by `impl From<S> for T`. See
/// [`TRAIT_INTO_UUID`]; it claims the slot after `Show`.
pub const TRAIT_FROM_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_0019);
/// Canonical identity of the prelude `Into<T>` conversion trait
/// (`core::convert::Into`): `fn into(self): T`, the use side of a
/// conversion. The solver satisfies `S: Into<T>` through `impl From<S> for
/// T` when no direct `Into` impl exists — sound at runtime because both
/// dictionaries are pinned by `validate_reserved_trait` to the same shape, a
/// 1-tuple of one 1-argument function (`from(value)` / `into(self)`), so a
/// `From` dictionary *is* an `Into` dictionary. That bridge keys on these
/// reserved uuids, exactly like operator desugaring keys on
/// [`TRAIT_ADD_UUID`].
pub const TRAIT_INTO_UUID: Uuid = Uuid::from_u128(0xffff_ffff_ffff_ffff_ffff_ffff_ffff_001a);

/// A reserved core trait: name/uuid pairs for the declarations in
/// `core_lib/traits.ab`, the trait analogue of [`super::Primitive`] /
/// [`super::Container`]. `validate_reserved_trait` pins a declaration
/// claiming either half to the canonical pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedTrait {
    /// `Add` — the `+` operator.
    Add,
    /// `Sub` — the `-` operator.
    Sub,
    /// `Mul` — the `*` operator.
    Mul,
    /// `Div` — the `/` operator.
    Div,
    /// `Mod` — the `%` operator.
    Mod,
    /// `Eq` — the `==`/`!=` operators.
    Eq,
    /// `Ord` — the `<`/`<=`/`>`/`>=` operators.
    Ord,
    /// `Default` — no operator; standard-library convenience.
    Default,
    /// `Show` — no operator; the stringifier that bounds `Exception::throw`.
    Show,
    /// `From<T>` — no operator; the declaration side of a conversion.
    From,
    /// `Into<T>` — no operator; the use side of a conversion, satisfiable
    /// through a `From` impl (the solver's uuid-anchored bridge).
    Into,
}

impl ReservedTrait {
    /// Every reserved core trait.
    pub const ALL: [Self; 11] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Mod,
        Self::Eq,
        Self::Ord,
        Self::Default,
        Self::Show,
        Self::From,
        Self::Into,
    ];

    /// The reserved identity uuid for this trait.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        match self {
            Self::Add => TRAIT_ADD_UUID,
            Self::Sub => TRAIT_SUB_UUID,
            Self::Mul => TRAIT_MUL_UUID,
            Self::Div => TRAIT_DIV_UUID,
            Self::Mod => TRAIT_MOD_UUID,
            Self::Eq => TRAIT_EQ_UUID,
            Self::Ord => TRAIT_ORD_UUID,
            Self::Default => TRAIT_DEFAULT_UUID,
            Self::Show => TRAIT_SHOW_UUID,
            Self::From => TRAIT_FROM_UUID,
            Self::Into => TRAIT_INTO_UUID,
        }
    }

    /// The canonical trait name, as spelled in `core_lib/traits.ab`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::Mul => "Mul",
            Self::Div => "Div",
            Self::Mod => "Mod",
            Self::Eq => "Eq",
            Self::Ord => "Ord",
            Self::Default => "Default",
            Self::Show => "Show",
            Self::From => "From",
            Self::Into => "Into",
        }
    }

    /// The number of trait-level type parameters the reserved trait declares
    /// (`From<T>`/`Into<T>` → 1, everything else 0). Part of the canonical
    /// shape `validate_reserved_trait` pins.
    #[must_use]
    pub const fn type_param_count(self) -> usize {
        match self {
            Self::From | Self::Into => 1,
            _ => 0,
        }
    }

    /// The reserved trait matching a uuid, if any.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.uuid() == uuid)
    }

    /// The reserved trait matching a canonical name, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait System Types
// ─────────────────────────────────────────────────────────────────────────────

/// A resolved trait bound (`T: Eq`, `T: From<String>`): the trait's identity
/// plus its resolved type arguments. Everything semantic keys off the uuid
/// and the argument types; the name is display only.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitBound {
    /// The bound trait's identity.
    pub trait_uuid: Uuid,
    /// The name the bound was written as (display only).
    pub name: Arc<str>,
    /// The bound's resolved type arguments (`From<String>` → `[String]`).
    /// Empty for the common argument-less bound. May contain [`Type::Param`]s
    /// referencing the enclosing declaration's own type parameters, and (in a
    /// scheme) [`Type::Var`]s standing for the scheme's quantified variables.
    pub args: Vec<Type>,
}

impl TraitBound {
    /// An argument-less bound.
    #[must_use]
    pub fn simple(trait_uuid: Uuid, name: impl Into<Arc<str>>) -> Self {
        Self {
            trait_uuid,
            name: name.into(),
            args: Vec::new(),
        }
    }
}

/// Definition of a trait.
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// The trait's nominal identity (its `unique(<uuid>)` prefix).
    pub uuid: Uuid,

    /// Trait name for display and in-scope lookup.
    pub name: Arc<str>,

    /// Trait-level type parameter names (`trait From<T>` → `["T"]`), in
    /// declaration order. Method signatures reference them as bare `Named`s,
    /// resolved per use site exactly like `Self` and method-level parameters.
    /// An impl of a parameterized trait spells one argument per entry, and
    /// coherence/dispatch key on those arguments' head identities.
    pub type_params: Vec<Arc<str>>,

    /// Associated type names (`type Error;`), in declaration order. Each
    /// impl binds every entry (`type Error = String;` →
    /// [`TraitImpl::assoc_bindings`]); method signatures reference them as
    /// [`super::Type::Projection`]s over `Self`, eliminated per use site by
    /// the dispatching impl's binding. No dictionary slot, no runtime
    /// representation — a checker-only vocabulary.
    pub assoc_types: Vec<Arc<str>>,

    /// The trait's fully-qualified location identity — the key the resolve
    /// pass canonicalizes every trait *reference* to, and the key the
    /// build-global [`TraitRegistry::uuid_for_fqn`] table indexes this trait
    /// under. `None` for a registry-less check (single-file/tests), where
    /// references stay bare and resolve by in-scope name instead.
    pub fqn: Option<Fqn>,

    /// Methods defined by this trait, in declaration order.
    pub methods: Vec<TraitMethodDef>,
}

impl TraitDef {
    /// Create a new trait definition (no location identity — see
    /// [`with_fqn`](Self::with_fqn)).
    #[must_use]
    pub fn new(uuid: Uuid, name: impl Into<Arc<str>>) -> Self {
        Self {
            uuid,
            name: name.into(),
            type_params: Vec::new(),
            assoc_types: Vec::new(),
            fqn: None,
            methods: Vec::new(),
        }
    }

    /// Attach the trait's own type parameter names (`trait From<T>`).
    #[must_use]
    pub fn with_type_params(mut self, type_params: Vec<Arc<str>>) -> Self {
        self.type_params = type_params;
        self
    }

    /// Attach the trait's associated type names (`type Error;`).
    #[must_use]
    pub fn with_assoc_types(mut self, assoc_types: Vec<Arc<str>>) -> Self {
        self.assoc_types = assoc_types;
        self
    }

    /// Attach the trait's fully-qualified location identity, so registering
    /// it also indexes the build-global `Fqn → uuid` table.
    #[must_use]
    pub fn with_fqn(mut self, fqn: Option<Fqn>) -> Self {
        self.fqn = fqn;
        self
    }

    /// Add a method.
    #[must_use]
    pub fn with_method(mut self, method: TraitMethodDef) -> Self {
        self.methods.push(method);
        self
    }

    /// The dictionary slot order for this trait: method indices sorted by
    /// method name. A bounded generic function compiles bound-method calls
    /// as tuple accesses into a dictionary argument, and call sites build
    /// that tuple from a concrete impl — both sides derive the layout from
    /// this one function, so they can never disagree within a build.
    /// (Cross-build agreement is the content-addressing story: dictionary
    /// construction and slot access are both *bytecode*, covered by the
    /// caller's and callee's hashes.)
    #[must_use]
    pub fn dictionary_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.methods.len()).collect();
        order.sort_by(|&a, &b| self.methods[a].name.cmp(&self.methods[b].name));
        order
    }

    /// The dictionary slot index of a method, by name. See
    /// [`dictionary_order`](Self::dictionary_order).
    #[must_use]
    pub fn dictionary_slot(&self, method_name: &str) -> Option<usize> {
        self.dictionary_order()
            .into_iter()
            .position(|idx| self.methods[idx].name.as_ref() == method_name)
    }
}

/// A method signature in a trait definition.
///
/// The signature is stored *un-instantiated*: parameter/return types keep
/// `Self` and any method-level type parameters (`U`) as bare `Named`s, and the
/// effect row keeps `E` unresolved. `type_param_names`/`ability_var_names`
/// record the method's own generics so each use site (a trait-dispatched call,
/// or an impl body) resolves the signature under its own scope — fresh
/// inference variables at a call site, rigid parameters plus a fresh row scope
/// in an impl body — instead of baking one set of variables into the registry.
///
/// A method-level type parameter may carry trait bounds (`fn tag<U: Eq>`).
/// Those thread through exactly like a bounded function's: a concrete-receiver
/// trait-dispatched call records one hidden dictionary per bound (in
/// [`method_bounds`](Self::method_bounds) / [`crate::ast::dict_params`] order)
/// and pushes it as a trailing argument, and the compiled impl method allocates
/// the matching trailing dictionary parameters. Dictionary-*mediated* dispatch
/// of a bounded method (a rigid-parameter receiver, or a conditional impl's
/// dictionary) is not yet supported and is rejected loudly.
#[derive(Debug, Clone)]
pub struct TraitMethodDef {
    /// Method name.
    pub name: Arc<str>,

    /// Whether the method takes `self` as first argument.
    pub has_self: bool,

    /// Parameter types (excluding self), with `Self`/`U`/`E` unresolved.
    pub params: Vec<Type>,

    /// Return type, with `Self`/`U`/`E` unresolved.
    pub ret: Type,

    /// The method's declared effect row (`with E` / `with Stdio`), unresolved.
    /// A name matching `ability_var_names` is the row's polymorphic tail.
    pub abilities: Vec<crate::ast::QualifiedName>,

    /// Method-level type parameter names (`fn tag<U>` → `["U"]`), bounded or
    /// not. Bounds are recorded separately in [`method_bounds`](Self::method_bounds).
    pub type_param_names: Vec<Arc<str>>,

    /// Method-level ability (row) variable names (`fn each<E!>` → `["E"]`).
    pub ability_var_names: Vec<Arc<str>>,

    /// The method's own trait bounds as `(param name, bound reference)`
    /// pairs, in [`crate::ast::dict_params`] order — the single authority the
    /// compiled impl method allocates its trailing dictionary parameters from,
    /// so a concrete-receiver call site's recorded dictionaries and the
    /// method's slots can never disagree. The bound's name carries the resolve
    /// pass's canonical `Fqn` (in [`crate::ast::QualifiedName::resolved`]), so
    /// a call site resolves it to the *defining* module's trait — never
    /// re-resolved in the caller's scope — and impl-method conformance
    /// compares by identity.
    pub method_bounds: Vec<(Arc<str>, crate::ast::TraitRef)>,
}

impl TraitMethodDef {
    /// Create a new trait method definition with no method-level generics.
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, has_self: bool, params: Vec<Type>, ret: Type) -> Self {
        Self {
            name: name.into(),
            has_self,
            params,
            ret,
            abilities: Vec::new(),
            type_param_names: Vec::new(),
            ability_var_names: Vec::new(),
            method_bounds: Vec::new(),
        }
    }

    /// Attach the method's declared effect row and method-level generics
    /// (type-parameter and ability-variable names, plus each type parameter's
    /// trait bounds in `dict_params` order).
    #[must_use]
    pub fn with_generics(
        mut self,
        abilities: Vec<crate::ast::QualifiedName>,
        type_param_names: Vec<Arc<str>>,
        ability_var_names: Vec<Arc<str>>,
        method_bounds: Vec<(Arc<str>, crate::ast::TraitRef)>,
    ) -> Self {
        self.abilities = abilities;
        self.type_param_names = type_param_names;
        self.ability_var_names = ability_var_names;
        self.method_bounds = method_bounds;
        self
    }
}

/// A registered trait implementation.
#[derive(Debug, Clone)]
pub struct TraitImpl {
    /// The identity of the trait being implemented.
    pub trait_uuid: Uuid,

    /// The nominal identity of the implementing type — its UUID. Coherence
    /// and dispatch key off this, so it is the same identity the inherent
    /// path derives (`inherent::impl_key_for(..).uuid()`): a declared struct,
    /// an `extern`/primitive nominal, or a declared/prelude enum all carry
    /// one. Impl targets without a UUID (unknown heads, bare parameters) are
    /// rejected before an impl is built.
    pub type_uuid: Uuid,

    /// The implementing type's head name, for diagnostics and `Debug` only —
    /// nothing semantic keys off it.
    pub type_name: Arc<str>,

    /// The impl's trait type arguments (`impl From<Number> for Money` →
    /// `[Number]`), resolved under the impl's rigid type parameters — so a
    /// conditional impl's argument may carry [`Type::Param`]s
    /// (`impl<T> From<List<T>> for …` → `[List<T>]`). Empty for an impl of
    /// an argument-less trait. Coherence and the dispatch symbol key on each
    /// argument's *head* identity ([`trait_arg_head`]); the solver refines
    /// with the full types at use sites, exactly like [`target`](Self::target).
    pub trait_args: Vec<Type>,

    /// Whether the impl block declares its own type parameters
    /// (`impl<T> Show for Wrapper<T>`). A generic (conditional) impl serves
    /// as a dictionary source by closing each method over the dictionaries
    /// *its* bounds demand — the solver derives those from [`target`] and
    /// [`bounds`] (see `Infer::solve_bound`).
    ///
    /// [`target`]: Self::target
    /// [`bounds`]: Self::bounds
    pub is_generic: bool,

    /// The impl's applied target shape, with the impl's type parameters as
    /// [`Type::Param`]s (`Pair<T>` → `Nominal(pair, Record{ Param(T), … })`,
    /// `Option<T>` → `Named(Option, [Param(T)], uuid)`). `None` for a
    /// non-generic impl. The solver unifies this against a concrete type to
    /// recover the impl's type-parameter assignments before solving
    /// [`bounds`](Self::bounds).
    pub target: Option<Type>,

    /// The impl's own trait bounds, in [`crate::ast::dict_params`] order —
    /// the same order the compiled impl methods take their hidden trailing
    /// dictionary parameters. Each `(param, bound)` becomes one inner
    /// dictionary the solver derives (against the assignment `param` unifies
    /// to) and each method closure forwards. Empty for a non-generic impl.
    pub bounds: Vec<(Arc<str>, super::TraitBound)>,

    /// Associated type bindings (`type Error = String;`): one entry per
    /// associated type the trait declares, resolved under the impl's rigid
    /// type parameters (so a conditional impl's binding may carry
    /// [`Type::Param`]s, recovered by the same target match as
    /// [`trait_args`](Self::trait_args)). Instantiation reads these to
    /// eliminate `Self::Error` projections; nothing at runtime does.
    pub assoc_bindings: Vec<(Arc<str>, Type)>,

    /// Method dispatch symbols: method name -> canonical impl-method symbol.
    ///
    /// The symbol (see [`impl_method_symbol`]) names the compiled method
    /// function; the compiler resolves it to a content-addressed hash the
    /// same way it resolves ordinary function names.
    pub methods: HashMap<Arc<str>, Arc<str>>,
}

impl TraitImpl {
    /// Create a new trait implementation for the nominal type identified by
    /// `type_uuid` (its head name `type_name` is display-only).
    #[must_use]
    pub fn new(trait_uuid: Uuid, type_uuid: Uuid, type_name: impl Into<Arc<str>>) -> Self {
        Self {
            trait_uuid,
            type_uuid,
            type_name: type_name.into(),
            trait_args: Vec::new(),
            is_generic: false,
            target: None,
            bounds: Vec::new(),
            assoc_bindings: Vec::new(),
            methods: HashMap::new(),
        }
    }

    /// Attach the impl's associated type bindings (`type Error = String;`).
    #[must_use]
    pub fn with_assoc_bindings(mut self, assoc_bindings: Vec<(Arc<str>, Type)>) -> Self {
        self.assoc_bindings = assoc_bindings;
        self
    }

    /// The impl's binding for an associated type, by name.
    #[must_use]
    pub fn assoc_binding(&self, name: &str) -> Option<&Type> {
        self.assoc_bindings
            .iter()
            .find(|(n, _)| n.as_ref() == name)
            .map(|(_, ty)| ty)
    }

    /// Attach the impl's trait type arguments (`impl From<Number> for …`).
    #[must_use]
    pub fn with_trait_args(mut self, trait_args: Vec<Type>) -> Self {
        self.trait_args = trait_args;
        self
    }

    /// Each trait argument's head identity, in order — the granularity
    /// coherence and the dispatch symbol key on. `None` if any argument has
    /// no nominal head (rejected at the declaration site, so registered
    /// impls always yield `Some`).
    #[must_use]
    pub fn arg_heads(&self) -> Option<Vec<Uuid>> {
        self.trait_args.iter().map(trait_arg_head).collect()
    }

    /// Mark this impl as a conditional (generic) impl, recording the applied
    /// target shape and the impl's own bounds so the solver can derive its
    /// dictionary. Passing an empty bound list is legal — a generic target
    /// with no bounds (`impl<T> Show for Wrapper<T>`).
    #[must_use]
    pub fn with_generic_target(
        mut self,
        target: Type,
        bounds: Vec<(Arc<str>, super::TraitBound)>,
    ) -> Self {
        self.is_generic = true;
        self.target = Some(target);
        self.bounds = bounds;
        self
    }

    /// Add a method implementation.
    #[must_use]
    pub fn with_method(mut self, name: impl Into<Arc<str>>, symbol: impl Into<Arc<str>>) -> Self {
        self.methods.insert(name.into(), symbol.into());
        self
    }
}

/// The head identity a trait argument keys coherence and dispatch on: the
/// argument's nominal head uuid. After `resolve_holes` every real head —
/// declared structs/enums, primitives, prelude enums, and the built-in
/// containers — carries a uuid (the same authority `impl_key_for` applies to
/// impl targets), so `None` means the argument cannot anchor an impl: a
/// structural type (record, tuple, function) or a bare type parameter.
/// Rejected at the impl declaration site.
#[must_use]
pub fn trait_arg_head(ty: &Type) -> Option<Uuid> {
    match ty {
        Type::Nominal(n) => Some(n.uuid),
        Type::Named(n) => n.uuid,
        _ => None,
    }
}

/// The canonical function symbol for an impl method.
///
/// Impl methods are compiled as ordinary named functions under this symbol,
/// so they flow through the same content-addressed hash finalization as any
/// other function. The symbol is derived only from source-stable identities —
/// the implementing type's UUID, the trait's UUID, the trait arguments' head
/// UUIDs, and the method name — never from names that can collide (two
/// same-named traits implemented for one type must not share a symbol) or
/// from compilation-order artifacts. The `::` separator cannot appear in
/// module-qualified names (which use `.`), so these symbols never collide
/// with user functions.
///
/// An argument-less trait's symbol keeps the historical
/// `<type>::<trait>::<method>` form exactly (pinned hashes like
/// `THROW_IMPL_HASH` must not move); a parameterized trait appends the
/// argument heads to the trait segment (`<type>::<trait><H1,H2>::<method>`),
/// which no argument-less symbol can collide with (`<`/`>` never appear in
/// uuids or method names).
#[must_use]
pub fn impl_method_symbol(
    type_uuid: &Uuid,
    trait_uuid: &Uuid,
    arg_heads: &[Uuid],
    method_name: &str,
) -> Arc<str> {
    let args = if arg_heads.is_empty() {
        String::new()
    } else {
        let heads: Vec<String> = arg_heads.iter().map(uuid_to_source).collect();
        format!("<{}>", heads.join(","))
    };
    format!(
        "{}::{}{args}::{method_name}",
        uuid_to_source(type_uuid),
        uuid_to_source(trait_uuid)
    )
    .into()
}

/// Render a UUID in Ambient's canonical source form: uppercase, hyphenated.
///
/// UUID literals are written uppercase in source; the `uuid` crate otherwise
/// renders them lowercase. Anywhere a UUID is shown to a user or embedded in a
/// symbol name — `unique(...)` type display and the `<uuid>::method` /
/// `<type-uuid>::<trait-uuid>::<method>` symbols — goes through this so the
/// rendered form matches the source syntax and round-trips.
#[must_use]
pub fn uuid_to_source(uuid: &Uuid) -> String {
    uuid.hyphenated().to_string().to_uppercase()
}

/// Result of looking up a method by name on a nominal type.
#[derive(Debug)]
pub enum MethodLookup<'a> {
    /// No trait implemented for the type provides the method.
    NotFound,
    /// Exactly one implementation provides the method.
    Found {
        /// The identity of the trait providing the method.
        trait_uuid: Uuid,
        /// The trait's method signature.
        method: &'a TraitMethodDef,
        /// The canonical dispatch symbol (see [`impl_method_symbol`]).
        symbol: Arc<str>,
    },
    /// Multiple traits implemented for the type provide a method with this
    /// name; the call must be disambiguated.
    Ambiguous {
        /// Names of the traits that provide the method.
        traits: Vec<Arc<str>>,
    },
    /// Exactly one trait provides the method, but through several impls
    /// differing in trait arguments (`impl From<Number> for X` and
    /// `impl From<String> for X` both provide `from`). The caller selects
    /// among [`TraitRegistry::impls_of`] by argument/return types.
    MultiImpl {
        /// The identity of the trait providing the method.
        trait_uuid: Uuid,
        /// The trait's method signature.
        method: &'a TraitMethodDef,
    },
}

/// Registry of trait definitions and implementations.
///
/// Definitions are keyed by the trait's identity uuid; `name_to_uuid` is the
/// in-scope lookup table (imports and locals register here, later
/// registrations shadowing earlier ones — the same precedence every other
/// name follows). Impls key on `(trait uuid, type uuid)` and, within that
/// pair, on the trait arguments' head identities — the coherence
/// granularity: an argument-less trait keeps one impl per type, and a
/// parameterized trait takes one impl per distinct argument-head list
/// (`From<Number>` and `From<String>` for one type coexist;
/// `From<Option<Number>>` and `From<Option<String>>` conflict).
#[derive(Debug, Clone, Default)]
pub struct TraitRegistry {
    /// Map from trait identity to trait definition.
    traits: HashMap<Uuid, TraitDef>,

    /// Map from in-scope trait name to identity.
    name_to_uuid: HashMap<Arc<str>, Uuid>,

    /// Build-global map from a trait's fully-qualified location identity to
    /// its uuid. The resolve pass canonicalizes every trait *reference* to
    /// an [`Fqn`]; the checker maps that `Fqn` here, so a bound resolves in
    /// the module that *defined* it — never re-resolved in the consumer's
    /// scope, and never shadowed by a consumer-side same-named trait. Every
    /// registered trait with a location identity indexes here, in-scope or
    /// not, so a foreign signature's bound resolves without importing.
    fqn_to_uuid: HashMap<Fqn, Uuid>,

    /// Map from (trait uuid, nominal type UUID) to implementations, one per
    /// distinct trait-argument head list (a single entry for argument-less
    /// traits).
    impls: HashMap<(Uuid, Uuid), Vec<TraitImpl>>,
}

impl TraitRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a trait definition, binding its name in scope.
    pub fn register_trait(&mut self, def: TraitDef) {
        self.name_to_uuid.insert(def.name.clone(), def.uuid);
        self.index_fqn(&def);
        self.traits.insert(def.uuid, def);
    }

    /// Register a trait definition *without* binding its bare name in
    /// scope — the identity resolves ([`get_trait`](Self::get_trait)) and
    /// its [`Fqn`] indexes the build-global table, but a local/imported
    /// trait of the same name still wins for *bare* in-scope lookups. Used
    /// for foreign traits a module never imported: their impls and the
    /// bounds of foreign signatures resolve by `Fqn`.
    pub fn register_trait_unnamed(&mut self, def: TraitDef) {
        self.index_fqn(&def);
        self.traits.entry(def.uuid).or_insert(def);
    }

    fn index_fqn(&mut self, def: &TraitDef) {
        if let Some(fqn) = &def.fqn {
            self.fqn_to_uuid.insert(fqn.clone(), def.uuid);
        }
    }

    /// Get trait definition by identity.
    #[must_use]
    pub fn get_trait(&self, uuid: Uuid) -> Option<&TraitDef> {
        self.traits.get(&uuid)
    }

    /// Resolve an *in-scope* trait name to its identity: locals and
    /// imports, later registrations shadowing earlier. This is the lookup
    /// for everything spelled in the current module — impl headers, local
    /// bounds — so trait definitions stay import-scoped (`Default` is
    /// unavailable without `use core::traits::Default`).
    #[must_use]
    pub fn lookup_trait(&self, name: &str) -> Option<Uuid> {
        self.name_to_uuid.get(name).copied()
    }

    /// Resolve a trait by its canonicalized location identity — the key the
    /// resolve pass wrote onto every trait reference. This is scope-blind by
    /// design: the `Fqn` already names the defining module, so a foreign
    /// signature's bound resolves to the same trait it did in its own
    /// module, and a consumer-side same-named trait cannot shadow it.
    #[must_use]
    pub fn uuid_for_fqn(&self, fqn: &Fqn) -> Option<Uuid> {
        self.fqn_to_uuid.get(fqn).copied()
    }

    /// Register a trait implementation.
    ///
    /// Returns the previously registered impl for the same
    /// `(trait, trait-argument heads, type)` key, if any — a coherence
    /// violation the caller should report.
    pub fn register_impl(&mut self, impl_: TraitImpl) -> Option<TraitImpl> {
        let key = (impl_.trait_uuid, impl_.type_uuid);
        let heads = impl_.arg_heads();
        let entries = self.impls.entry(key).or_default();
        if let Some(pos) = entries.iter().position(|e| e.arg_heads() == heads) {
            let previous = std::mem::replace(&mut entries[pos], impl_);
            return Some(previous);
        }
        entries.push(impl_);
        None
    }

    /// Get the implementation for a trait and nominal type, when exactly one
    /// is registered — always the case for argument-less traits (their
    /// coherence key has one slot per type). `None` when there is no impl
    /// *or* several argument-differing impls (callers that can disambiguate
    /// go through [`impls_of`](Self::impls_of)).
    #[must_use]
    pub fn get_impl(&self, trait_uuid: Uuid, type_uuid: Uuid) -> Option<&TraitImpl> {
        match self.impls.get(&(trait_uuid, type_uuid))?.as_slice() {
            [single] => Some(single),
            _ => None,
        }
    }

    /// Every implementation of `trait_uuid` for `type_uuid`, one per
    /// distinct trait-argument head list. Sorted by argument heads so
    /// iteration is deterministic.
    #[must_use]
    pub fn impls_of(&self, trait_uuid: Uuid, type_uuid: Uuid) -> Vec<&TraitImpl> {
        let mut impls: Vec<&TraitImpl> = self
            .impls
            .get(&(trait_uuid, type_uuid))
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        impls.sort_by_key(|impl_| impl_.arg_heads());
        impls
    }

    /// Every implementation of `trait_uuid` whose *first* trait argument's
    /// head is `arg_head`, across all implementing types — the `Into`
    /// bridge's reverse lookup (`S: Into<?>` scans `impl From<S> for _`).
    /// Sorted by implementing type then argument heads, so candidate
    /// enumeration is deterministic.
    #[must_use]
    pub fn impls_with_first_arg(&self, trait_uuid: Uuid, arg_head: Uuid) -> Vec<&TraitImpl> {
        let mut impls: Vec<&TraitImpl> = self
            .impls
            .iter()
            .filter(|((t, _), _)| *t == trait_uuid)
            .flat_map(|(_, v)| v.iter())
            .filter(|impl_| {
                impl_
                    .trait_args
                    .first()
                    .and_then(trait_arg_head)
                    .is_some_and(|h| h == arg_head)
            })
            .collect();
        impls.sort_by_key(|impl_| (impl_.type_uuid, impl_.arg_heads()));
        impls
    }

    /// Find all implementations for a nominal type.
    ///
    /// Sorted by trait uuid then argument heads so lookups are deterministic
    /// (the backing map has arbitrary iteration order).
    #[must_use]
    pub fn impls_for_type(&self, type_uuid: Uuid) -> Vec<&TraitImpl> {
        let mut impls: Vec<&TraitImpl> = self
            .impls
            .iter()
            .filter(|((_, uuid), _)| *uuid == type_uuid)
            .flat_map(|(_, v)| v.iter())
            .collect();
        impls.sort_by_key(|impl_| (impl_.trait_uuid, impl_.arg_heads()));
        impls
    }

    /// Find a method by name for a given nominal type.
    #[must_use]
    pub fn find_method(&self, type_uuid: Uuid, method_name: &str) -> MethodLookup<'_> {
        let mut matches: Vec<(Uuid, &TraitMethodDef, Arc<str>)> = Vec::new();
        for impl_ in self.impls_for_type(type_uuid) {
            if let Some(symbol) = impl_.methods.get(method_name)
                && let Some(trait_def) = self.get_trait(impl_.trait_uuid)
                && let Some(method) = trait_def
                    .methods
                    .iter()
                    .find(|m| m.name.as_ref() == method_name)
            {
                matches.push((impl_.trait_uuid, method, Arc::clone(symbol)));
            }
        }

        if matches.is_empty() {
            return MethodLookup::NotFound;
        }
        if matches.len() == 1 {
            // Vec::swap_remove on a single-element vec cannot fail.
            let (trait_uuid, method, symbol) = matches.swap_remove(0);
            return MethodLookup::Found {
                trait_uuid,
                method,
                symbol,
            };
        }
        // Several impls of *one* trait (argument-differing): the caller
        // disambiguates by argument/return types via `impls_of`.
        let first = matches[0].0;
        if matches.iter().all(|(t, _, _)| *t == first) {
            return MethodLookup::MultiImpl {
                trait_uuid: first,
                method: matches[0].1,
            };
        }
        let mut traits: Vec<Arc<str>> = matches
            .iter()
            .filter_map(|(id, _, _)| self.get_trait(*id).map(|t| Arc::clone(&t.name)))
            .collect();
        traits.dedup();
        MethodLookup::Ambiguous { traits }
    }

    /// Check if a type implements a trait (at any trait arguments).
    #[must_use]
    pub fn implements(&self, type_uuid: Uuid, trait_uuid: Uuid) -> bool {
        self.impls
            .get(&(trait_uuid, type_uuid))
            .is_some_and(|v| !v.is_empty())
    }
}
