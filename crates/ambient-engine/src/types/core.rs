//! The [`Type`] enum, its support structs, [`TypeVarGen`], and the
//! convenience constructors/accessors.

use std::sync::Arc;

use uuid::Uuid;

use super::{
    AbilityId, AbilitySet, AbilityVarId, BINARY_UUID, BOOL_UUID, LIST_UUID, MAP_UUID, NUMBER_UUID,
    OPTION_UUID, Primitive, RESULT_UUID, SET_UUID, STRING_UUID, TypeVarId,
};

/// Counter for generating fresh type variable IDs.
#[derive(Debug, Default)]
pub struct TypeVarGen {
    next_id: TypeVarId,
    next_ability_id: AbilityVarId,
}

impl TypeVarGen {
    /// Create a new type variable generator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 0,
            next_ability_id: 0,
        }
    }

    /// Generate a fresh type variable.
    pub fn fresh(&mut self) -> Type {
        let id = self.next_id;
        self.next_id += 1;
        Type::Var(id)
    }

    /// Generate a fresh type variable ID.
    pub fn fresh_id(&mut self) -> TypeVarId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Generate a fresh ability variable.
    pub fn fresh_ability_var(&mut self) -> AbilitySet {
        let id = self.next_ability_id;
        self.next_ability_id += 1;
        AbilitySet::Var(id)
    }

    /// Generate a fresh ability variable ID.
    pub fn fresh_ability_id(&mut self) -> AbilityVarId {
        let id = self.next_ability_id;
        self.next_ability_id += 1;
        id
    }
}

/// Represents a type in the Ambient language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    // ─────────────────────────────────────────────────────────────────────────
    // Primitive types
    // ─────────────────────────────────────────────────────────────────────────
    /// Unit type `()`, represents absence of a meaningful value.
    Unit,

    // The primitives `Bool`/`Number`/`String`/`Binary` are not variants here:
    // they are nominal `Named` types carrying reserved uuids (see
    // `Type::bool`/`number`/`string`/`bytes` and `BOOL_UUID` etc.), uniform
    // with how `Option`/`Result` work. Match them with `Type::as_primitive`.

    // ─────────────────────────────────────────────────────────────────────────
    // Composite types
    // ─────────────────────────────────────────────────────────────────────────
    /// Tuple type: fixed-size, heterogeneous collection.
    /// `(T1, T2, ..., Tn)`
    Tuple(Vec<Type>),

    /// Record type: named fields with types (structural typing).
    /// `{ field1: T1, field2: T2, ... }`
    Record(RecordType),

    /// Function type: parameters -> return type with abilities.
    /// `(P1, P2, ...) -> R with A1, A2, ...`
    Function(FunctionType),

    // ─────────────────────────────────────────────────────────────────────────
    // Polymorphism
    // ─────────────────────────────────────────────────────────────────────────
    /// A type variable used during inference.
    Var(TypeVarId),

    /// A rigid type parameter, named by the source identifier it was
    /// introduced under (`T` in `fn f<T>(x: T)`). Distinct from
    /// [`Type::Var`] (a *flexible* inference variable) and from an
    /// unresolved [`Type::Named`] (a *nominal reference*): a `Param` is an
    /// atom that unifies only with the identically-named `Param`, so
    /// rigidity is structural — `generalize` never quantifies it (it holds
    /// no free inference vars) and it survives into a function/method body
    /// for diagnostics.
    ///
    /// Only body checking converts a written `T` annotation into a `Param`
    /// (see `Infer::resolve_holes`, gated by `Infer::rigid_params`);
    /// signature-scheme paths substitute type parameters to fresh `Var`
    /// before quantifying, so a `Param` never reaches a signature hash.
    Param(Arc<str>),

    /// A quantified (forall) type scheme.
    /// `forall a b. (a -> b) -> List<a> -> List<b>`
    Forall(ForallType),

    // ─────────────────────────────────────────────────────────────────────────
    // Named types
    // ─────────────────────────────────────────────────────────────────────────
    /// A named type constructor with optional type arguments.
    /// `List<T>`, `Option<T>`, `Map<K, V>`
    Named(NamedType),

    /// A nominal type distinguished by UUID, incompatible with structurally
    /// identical types.
    /// `unique(uuid) struct UserId { value: string }`
    Nominal(NominalType),

    // ─────────────────────────────────────────────────────────────────────────
    // Handler types (Milestone 13)
    // ─────────────────────────────────────────────────────────────────────────
    /// A handler value type: `Handler<A>`
    /// Represents a first-class handler that can handle ability `A`.
    Handler(HandlerType),

    /// The unresolved surface form of a `Handler<A, R>` annotation — see
    /// [`HandlerAnnotationType`]. `Handler` is type *syntax* (like function
    /// arrows and tuples), not a nominal name: its first argument `A` is an
    /// **ability** reference, not a type. The parser recognizes the form and
    /// lowers it here; [`Infer::resolve_holes`](crate::infer::Infer::resolve_holes)
    /// resolves `A` under the ability-namespace policy and rewrites this to
    /// [`Type::Handler`]. It never survives type checking.
    HandlerAnnotation(HandlerAnnotationType),

    // ─────────────────────────────────────────────────────────────────────────
    // Special types
    // ─────────────────────────────────────────────────────────────────────────
    /// The never type `!`, for expressions that never return.
    Never,

    /// Error type used during type checking to allow recovery.
    Error,

    /// A type hole `_` for partial annotation.
    /// During inference, this is replaced with a fresh type variable.
    Hole,
}

/// A record type with named fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordType {
    /// Fields sorted by name for consistent comparison.
    pub fields: Vec<(Arc<str>, Type)>,
}

impl RecordType {
    /// Create a new record type with the given fields.
    /// Fields are sorted by name for consistent structural comparison.
    #[must_use]
    pub fn new(mut fields: Vec<(Arc<str>, Type)>) -> Self {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        Self { fields }
    }

    /// Get the type of a field by name.
    #[must_use]
    pub fn get_field(&self, name: &str) -> Option<&Type> {
        self.fields
            .binary_search_by(|(n, _)| n.as_ref().cmp(name))
            .ok()
            .map(|idx| &self.fields[idx].1)
    }
}

/// A function type with parameters, return type, and ability requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    /// Parameter types.
    pub params: Vec<Type>,

    /// Return type.
    pub ret: Box<Type>,

    /// Abilities required by this function (Milestone 8).
    /// Empty means the function is pure.
    pub abilities: AbilitySet,
}

impl FunctionType {
    /// Create a new pure function type (no abilities).
    #[must_use]
    pub fn new(params: Vec<Type>, ret: Type) -> Self {
        Self {
            params,
            ret: Box::new(ret),
            abilities: AbilitySet::Empty,
        }
    }

    /// Create a new function type with abilities.
    #[must_use]
    pub fn with_abilities(params: Vec<Type>, ret: Type, abilities: AbilitySet) -> Self {
        Self {
            params,
            ret: Box::new(ret),
            abilities,
        }
    }

    /// Check if this function is pure (has no abilities).
    #[must_use]
    pub fn is_pure(&self) -> bool {
        self.abilities.is_pure()
    }
}

/// A handler value type: `Handler<A, R>`
///
/// Represents a first-class handler that can handle a specific ability.
/// Handler values can be passed around, stored, and composed.
///
/// `answer` (`R`) is the type an arm yields when it *returns without
/// resuming* — equivalently, the result type of the handle expression this
/// handler is installed at. An always-resuming handler leaves `R` a free
/// variable (generalizable, so it unifies with whatever result each use
/// site requires); a non-resuming arm (`throw(e) => e * 2`) pins `R` to a
/// concrete type, which the handle site must then match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerType {
    /// The ability that this handler handles.
    /// This is a single ability ID (not a set), as handlers handle one ability at a time.
    pub ability: AbilityId,
    /// The answer type `R`: what an arm produces when it returns without
    /// resuming (== the handle expression's result type).
    pub answer: Box<Type>,
}

/// The unresolved surface form of `Handler<A, R>` — see
/// [`Type::HandlerAnnotation`]. Holds the ability *reference* `A` (not yet an
/// id) and the optional answer type `R`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerAnnotationType {
    /// The ability reference `A` as written, `::`-joined if qualified
    /// (`Stdio`, `core::system::Stdio`). Resolved through the ability
    /// namespace, not the type namespace.
    pub ability: Arc<str>,
    /// The answer type `R`, or `None` when omitted (`Handler<A>` means "R
    /// inferred"), in which case the checker mints a fresh variable.
    pub answer: Option<Box<Type>>,
}

impl HandlerType {
    /// Create a new handler type.
    #[must_use]
    pub fn new(ability: AbilityId, answer: Type) -> Self {
        Self {
            ability,
            answer: Box::new(answer),
        }
    }
}

/// A quantified type scheme (forall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForallType {
    /// Bound type variable IDs.
    pub vars: Vec<TypeVarId>,

    /// Bound ability variable IDs (Milestone 8).
    pub ability_vars: Vec<AbilityVarId>,

    /// The quantified type.
    pub body: Box<Type>,
}

impl ForallType {
    /// Create a new forall type.
    #[must_use]
    pub fn new(vars: Vec<TypeVarId>, body: Type) -> Self {
        Self {
            vars,
            ability_vars: Vec::new(),
            body: Box::new(body),
        }
    }

    /// Create a forall type with ability variables.
    #[must_use]
    pub fn with_abilities(
        vars: Vec<TypeVarId>,
        ability_vars: Vec<AbilityVarId>,
        body: Type,
    ) -> Self {
        Self {
            vars,
            ability_vars,
            body: Box::new(body),
        }
    }
}

/// A named type constructor (like `List<T>` or `Option<T>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    /// The type constructor name.
    pub name: Arc<str>,

    /// Type arguments (empty for non-generic types).
    pub args: Vec<Type>,

    /// Nominal identity of an enum.
    ///
    /// Every enum carries a `Some(uuid)`: a declared enum takes it from its
    /// mandatory `unique(<uuid>)` prefix, and the reserved-name prelude enums
    /// `Option`/`Result` take the fixed [`OPTION_UUID`]/[`RESULT_UUID`]. So two
    /// structurally identical enums are distinct types, even same-named enums
    /// in different packages. The built-in containers (`List`, `Map`, `Set`)
    /// likewise carry their reserved uuids ([`LIST_UUID`] etc.), so their
    /// applied form dispatches by uuid like every other nominal type. Only
    /// type-parameter references carry `None`: their identity *is* the head
    /// name.
    ///
    /// A `None` here is still a wildcard in unification (see `Infer::unify`),
    /// but only meaningfully for those structural/parameter names: any `None`
    /// on a *registered* enum name is resolved to that enum's canonical uuid
    /// before comparison, so an unresolved annotation or self-referential
    /// payload unifies strictly with the resolved, uuid-carrying form while two
    /// genuinely distinct enums never unify.
    pub uuid: Option<Uuid>,
}

impl NamedType {
    /// Create a new structural named type (no nominal identity).
    #[must_use]
    pub fn new(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self {
            name: name.into(),
            args,
            uuid: None,
        }
    }

    /// Create a non-generic structural named type.
    #[must_use]
    pub fn simple(name: impl Into<Arc<str>>) -> Self {
        Self::new(name, Vec::new())
    }

    /// Create a named type carrying a nominal identity (a declared enum).
    #[must_use]
    pub fn with_identity(name: impl Into<Arc<str>>, args: Vec<Type>, uuid: Option<Uuid>) -> Self {
        Self {
            name: name.into(),
            args,
            uuid,
        }
    }

    /// Rebuild this named type with new arguments, preserving its head name
    /// and nominal identity. Used at every site that maps a transformation
    /// over the arguments (substitution, hole resolution) so an enum's
    /// identity survives.
    #[must_use]
    pub fn map_args(&self, args: Vec<Type>) -> Self {
        Self {
            name: Arc::clone(&self.name),
            args,
            uuid: self.uuid,
        }
    }
}

/// A nominal type distinguished by UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalType {
    /// Unique identifier making this type distinct from structurally
    /// identical types.
    pub uuid: Uuid,

    /// The underlying structural type.
    pub inner: Box<Type>,

    /// Optional human-readable name for error messages.
    pub name: Option<Arc<str>>,

    /// Whether this type is `extern`: engine-provided, so Ambient code may name
    /// it and read its fields but may not construct it. A property of the nominal
    /// identity, so it travels with the type through substitution, unification,
    /// and cross-module resolution.
    pub is_extern: bool,
}

impl NominalType {
    /// Create a new nominal type. Non-`extern` by default; use
    /// [`with_extern`](Self::with_extern) to mark it engine-provided.
    #[must_use]
    pub fn new(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self {
            uuid,
            inner: Box::new(inner),
            name: name.map(Into::into),
            is_extern: false,
        }
    }

    /// Mark this nominal type as `extern` (or not), preserving everything else.
    #[must_use]
    pub fn with_extern(mut self, is_extern: bool) -> Self {
        self.is_extern = is_extern;
        self
    }

    /// Rebuild this nominal type with a new inner type, preserving its identity
    /// (`uuid`, `name`, and `is_extern`). Used at every site that maps a
    /// transformation over the inner type (substitution, hole resolution,
    /// unification) so the nominal identity survives.
    #[must_use]
    pub fn map_inner(&self, inner: Type) -> Self {
        Self {
            uuid: self.uuid,
            inner: Box::new(inner),
            name: self.name.clone(),
            is_extern: self.is_extern,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Type constructors (convenience methods)
// ─────────────────────────────────────────────────────────────────────────────

impl Type {
    /// Create a pure function type (no abilities).
    #[must_use]
    pub fn function(params: Vec<Type>, ret: Type) -> Self {
        Self::Function(FunctionType::new(params, ret))
    }

    /// Create a function type with abilities.
    #[must_use]
    pub fn function_with_abilities(params: Vec<Type>, ret: Type, abilities: AbilitySet) -> Self {
        Self::Function(FunctionType::with_abilities(params, ret, abilities))
    }

    /// Create a handler type: `Handler<A, R>`.
    #[must_use]
    pub fn handler(ability: AbilityId, answer: Type) -> Self {
        Self::Handler(HandlerType::new(ability, answer))
    }

    /// Create a tuple type.
    #[must_use]
    pub fn tuple(elements: Vec<Type>) -> Self {
        Self::Tuple(elements)
    }

    /// Create a record type.
    #[must_use]
    pub fn record(fields: impl IntoIterator<Item = (impl Into<Arc<str>>, Type)>) -> Self {
        let fields: Vec<_> = fields.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Self::Record(RecordType::new(fields))
    }

    /// Create a named type with arguments.
    #[must_use]
    pub fn named(name: impl Into<Arc<str>>, args: Vec<Type>) -> Self {
        Self::Named(NamedType::new(name, args))
    }

    /// Create a simple named type (no arguments).
    #[must_use]
    pub fn named_simple(name: impl Into<Arc<str>>) -> Self {
        Self::Named(NamedType::simple(name))
    }

    /// The `Bool` primitive, a nominal type carrying its reserved identity.
    #[must_use]
    pub fn bool() -> Self {
        Self::primitive_nominal(BOOL_UUID, "Bool")
    }

    /// The `Number` primitive, a nominal type carrying its reserved identity.
    #[must_use]
    pub fn number() -> Self {
        Self::primitive_nominal(NUMBER_UUID, "Number")
    }

    /// The `String` primitive, a nominal type carrying its reserved identity.
    #[must_use]
    pub fn string() -> Self {
        Self::primitive_nominal(STRING_UUID, "String")
    }

    /// The `Binary` primitive, a nominal type carrying its reserved identity.
    #[must_use]
    pub fn binary() -> Self {
        Self::primitive_nominal(BINARY_UUID, "Binary")
    }

    /// Build the canonical `extern` [`Type::Nominal`] for a primitive. This is
    /// value-identical to what a unit `extern` struct lowers to (a fieldless
    /// record wrapped in a nominal identity, marked `extern`), so the anchor and
    /// the source declaration in `core_lib` unify trivially. Primitives are the
    /// only `extern` types whose value is withheld yet whose literals still exist
    /// (via the compile-time anchors below).
    #[must_use]
    fn primitive_nominal(uuid: Uuid, name: &'static str) -> Self {
        Self::Nominal(
            NominalType::new(
                uuid,
                Type::Record(RecordType { fields: vec![] }),
                Some(name),
            )
            .with_extern(true),
        )
    }

    /// If this type is a primitive (a nominal type carrying a reserved primitive
    /// uuid), return which one. Mirrors [`as_option`](Self::as_option).
    #[must_use]
    pub fn as_primitive(&self) -> Option<Primitive> {
        match self {
            Self::Named(n) => n.uuid.and_then(Primitive::from_uuid),
            Self::Nominal(n) => Primitive::from_uuid(n.uuid),
            _ => None,
        }
    }

    /// Create an `Option<T>` type carrying its canonical nominal identity.
    #[must_use]
    pub fn option(inner: Type) -> Self {
        Self::Named(NamedType::with_identity(
            "Option",
            vec![inner],
            Some(OPTION_UUID),
        ))
    }

    /// Create a `Result<T, E>` type carrying its canonical nominal identity.
    #[must_use]
    pub fn result(ok: Type, err: Type) -> Self {
        Self::Named(NamedType::with_identity(
            "Result",
            vec![ok, err],
            Some(RESULT_UUID),
        ))
    }

    /// Create a `List<T>` type carrying its canonical nominal identity. The
    /// reserved uuid is what routes `list.method()` to `impl<T> List<T>` via a
    /// uuid-keyed dispatch symbol, exactly like a scalar primitive's methods.
    #[must_use]
    pub fn list(inner: Type) -> Self {
        Self::Named(NamedType::with_identity(
            "List",
            vec![inner],
            Some(LIST_UUID),
        ))
    }

    /// Create a `Map<K, V>` type carrying its canonical nominal identity. See
    /// [`list`](Self::list).
    #[must_use]
    pub fn map(key: Type, value: Type) -> Self {
        Self::Named(NamedType::with_identity(
            "Map",
            vec![key, value],
            Some(MAP_UUID),
        ))
    }

    /// Create a `Set<T>` type carrying its canonical nominal identity. See
    /// [`list`](Self::list).
    #[must_use]
    pub fn set(inner: Type) -> Self {
        Self::Named(NamedType::with_identity("Set", vec![inner], Some(SET_UUID)))
    }

    /// Check if this type is `Option<T>` and return the inner type.
    #[must_use]
    pub fn as_option(&self) -> Option<&Type> {
        match self {
            Self::Named(n) if n.name.as_ref() == "Option" && n.args.len() == 1 => Some(&n.args[0]),
            _ => None,
        }
    }

    /// Check if this type is `Result<T, E>` and return the ok and error types.
    #[must_use]
    pub fn as_result(&self) -> Option<(&Type, &Type)> {
        match self {
            Self::Named(n) if n.name.as_ref() == "Result" && n.args.len() == 2 => {
                Some((&n.args[0], &n.args[1]))
            }
            _ => None,
        }
    }

    /// Check if this type is a `List<T>` and return the element type.
    #[must_use]
    pub fn as_list(&self) -> Option<&Type> {
        match self {
            Self::Named(n) if n.name.as_ref() == "List" && n.args.len() == 1 => Some(&n.args[0]),
            _ => None,
        }
    }

    /// Create an unbound type variable.
    #[must_use]
    pub fn var(id: TypeVarId) -> Self {
        Self::Var(id)
    }

    /// Create a nominal type.
    #[must_use]
    pub fn nominal(uuid: Uuid, inner: Type, name: Option<impl Into<Arc<str>>>) -> Self {
        Self::Nominal(NominalType::new(uuid, inner, name))
    }

    /// Create a forall (polymorphic) type.
    #[must_use]
    pub fn forall(vars: Vec<TypeVarId>, body: Type) -> Self {
        if vars.is_empty() {
            body
        } else {
            Self::Forall(ForallType::new(vars, body))
        }
    }
}
