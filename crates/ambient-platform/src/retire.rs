//! Generation retirement: the trace that answers "when is the upgrade
//! finished?" (see `ref/live-upgrade.md`, "Retirement").
//!
//! Old code is reachable from exactly three kinds of place, all owned by
//! the runtime: values in `State` cells, values in the task registry (a
//! task's body or current-pass resolution), and in-flight frames. The
//! trace takes those as **roots**, walks every hash a root value can
//! reach (closure environments, handler arms, continuation frames, then
//! each function's static `dependencies`), and attributes each reachable
//! hash to the **latest generation that shipped it**. A generation none
//! of whose latest-shipped hashes are reachable is **retired** —
//! permanently: the runtime records it, `ambient store gc` may purge it,
//! and the dev loop reports the transition once.
//!
//! In-flight frames are sampled at boundaries rather than read from live
//! VMs (a running VM belongs to its thread): a task publishes the hash it
//! resolved for the current pass, and everything else a frame can hold is
//! built from values that are themselves rooted (cells, the registry, the
//! current name table).
//!
//! Attribution by *latest* shipper is what makes unchanged code harmless:
//! a full build re-ships every unchanged hash, so those hashes attribute
//! to the new generation and an old generation stays live only while a
//! hash it alone still owns — code that was changed away from — is
//! reachable. That is exactly "the upgrade is not finished".

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use ambient_ability::Value;

use crate::deploy::{Generation, Loaded};

/// Where a trace root lives — the provenance half of a pin diagnostic
/// ("generation 1 pinned by cell `conn-42`").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootOrigin {
    /// A `State` cell's current value.
    Cell(Arc<str>),
    /// A task registry entry: its body value or current-pass hash.
    Task(Arc<str>),
    /// A name still bound in the current table (late-bound resolution
    /// can reach it at any time).
    Name(Arc<str>),
}

impl std::fmt::Display for RootOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cell(name) => write!(f, "cell \"{name}\""),
            Self::Task(name) => write!(f, "task `{name}`"),
            Self::Name(name) => write!(f, "name `{name}`"),
        }
    }
}

/// One trace root: a runtime-held value and where it lives.
#[derive(Clone, Debug)]
pub struct Root {
    pub origin: RootOrigin,
    pub value: Value,
}

impl Root {
    /// A root that is a bare code hash (a task's current-pass
    /// resolution, a name-table binding).
    #[must_use]
    pub fn from_hash(origin: RootOrigin, hash: blake3::Hash) -> Self {
        Self {
            origin,
            value: Value::FunctionRef(hash),
        }
    }
}

/// A registry's contribution of trace roots, registered on the deploy
/// runtime by each client (the task runtime). Called with no other
/// locks held; implementations should lock briefly, clone, and return.
pub type RootProvider = Arc<dyn Fn() -> Vec<Root> + Send + Sync>;

/// Every code hash a value can reach directly: function refs, closure
/// code (then its environment), handler arms (then their captures),
/// ability-method default implementations, suspended-ability arguments,
/// and captured continuation frames. Modeled on the engine's `hash_value`
/// traversal, but emitting hashes instead of hashing — and walking
/// continuations, which content hashing deliberately treats as opaque.
pub fn value_code_hashes(value: &Value, out: &mut Vec<blake3::Hash>) {
    match value {
        Value::FunctionRef(hash) | Value::ObjectRef(hash) => out.push(*hash),
        Value::Closure(c) => {
            out.push(c.function_hash);
            for value in &c.environment {
                value_code_hashes(value, out);
            }
        }
        Value::Handler(h) => {
            out.extend(h.methods.values().copied());
            for value in &h.captures {
                value_code_hashes(value, out);
            }
        }
        Value::AbilityMethodRef(m) => out.extend(m.impl_fn),
        Value::SuspendedAbility(s) => {
            out.extend(s.impl_fn);
            for value in &s.args {
                value_code_hashes(value, out);
            }
        }
        Value::Continuation(k) => {
            for value in &k.stack {
                value_code_hashes(value, out);
            }
            for frame in &k.frames {
                out.push(frame.function_hash);
                for value in &frame.captures {
                    value_code_hashes(value, out);
                }
            }
            for handler in &k.handlers {
                out.extend(handler.handler.methods.values().copied());
                for value in &handler.handler.captures {
                    value_code_hashes(value, out);
                }
            }
        }
        Value::Tuple(values) | Value::List(values) => {
            for value in values.iter() {
                value_code_hashes(value, out);
            }
        }
        Value::Record(fields) => {
            for value in fields.values() {
                value_code_hashes(value, out);
            }
        }
        Value::Map(map) => {
            for (key, value) in &map.entries {
                value_code_hashes(key, out);
                value_code_hashes(value, out);
            }
        }
        Value::Set(set) => {
            for value in &set.elements {
                value_code_hashes(value, out);
            }
        }
        Value::Enum(e) => {
            if let Some(payload) = &e.payload {
                value_code_hashes(payload, out);
            }
        }
        Value::Unit
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Binary(_)
        | Value::AbilityRef(_)
        | Value::Module(_)
        | Value::ModuleMember(_) => {}
    }
}

/// The reachable set of one trace, with provenance: each hash maps to
/// the index of the root that first reached it (BFS order, so a pin
/// names a *direct-ish* holder rather than an arbitrary one).
pub(crate) struct Reach {
    pub(crate) seen: HashMap<blake3::Hash, usize>,
}

/// Trace reachability from per-root seed hashes through the cumulative
/// object stores: a function hash expands to its static `dependencies`
/// *and* every hash its constant pool mentions (the builder does not
/// mirror a bare function-as-value `PushConst` into `dependencies`, so
/// constants are walked too); a `const` value object expands to the
/// hashes inside its value; a native object is a leaf.
pub(crate) fn reach(loaded: &Loaded, seeds: &[(usize, Vec<blake3::Hash>)]) -> Reach {
    let mut seen: HashMap<blake3::Hash, usize> = HashMap::new();
    let mut queue: VecDeque<blake3::Hash> = VecDeque::new();
    for (root, hashes) in seeds {
        for hash in hashes {
            if !seen.contains_key(hash) {
                seen.insert(*hash, *root);
                queue.push_back(*hash);
            }
        }
    }
    while let Some(hash) = queue.pop_front() {
        let root = seen[&hash];
        let mut next = Vec::new();
        if let Some(func) = loaded.functions.get(&hash) {
            next.extend(func.dependencies.iter().copied());
            for constant in &func.constants {
                value_code_hashes(constant, &mut next);
            }
        } else if let Some(value) = loaded.values.get(&hash) {
            value_code_hashes(value, &mut next);
        }
        for hash in next {
            if let std::collections::hash_map::Entry::Vacant(entry) = seen.entry(hash) {
                entry.insert(root);
                queue.push_back(hash);
            }
        }
    }
    Reach { seen }
}

/// How a hash is labeled in diagnostics: by the name it was deployed
/// under, or by the named function whose body constructs/contains it (a
/// lambda has no deployed name; naming its innermost named ancestor is
/// what makes "pinned by cell `conn-42` (closure of `handle_connection`)"
/// possible).
#[derive(Clone, Debug)]
struct Label {
    name: Arc<str>,
    /// False when inherited from a named ancestor rather than bound
    /// directly.
    direct: bool,
}

/// One pinned hash: the root that reached it and what to call it.
#[derive(Clone)]
pub struct Pin {
    /// Where the pinning reference lives.
    pub root: RootOrigin,
    /// The pinned hash itself.
    pub hash: blake3::Hash,
    /// Human name for the hash, if one is known.
    label: Option<Label>,
}

impl Pin {
    /// Render the pinned hash for diagnostics: its deployed name, its
    /// named ancestor (`closure of <name>`), or a hash prefix.
    #[must_use]
    pub fn describe(&self) -> String {
        describe_hash(self.label.as_ref(), &self.hash)
    }
}

/// Render a hash for diagnostics by its label: deployed name, named
/// ancestor, or a hash prefix.
fn describe_hash(label: Option<&Label>, hash: &blake3::Hash) -> String {
    match label {
        Some(label) if label.direct => format!("`{}`", label.name),
        Some(label) => format!("closure of `{}`", label.name),
        None => format!("fn {}", &hash.to_hex().as_str()[..12]),
    }
}

/// A generation still kept alive by reachable hashes it alone ships.
#[derive(Debug)]
pub struct PinnedGeneration {
    pub id: u64,
    /// What pins it, one entry per pinned hash (BFS provenance).
    pub pins: Vec<Pin>,
}

impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} pinned by {}", self.describe(), self.root)
    }
}

/// One retirement trace's outcome — the programmatic query surface
/// (`DeployRuntime::retirement`).
#[derive(Debug, Default)]
pub struct RetirementReport {
    /// The current (most recently deployed) generation, if any deploy
    /// has happened. Never listed as retired or pinned.
    pub current: Option<u64>,
    /// Generations whose retirement this trace discovered.
    pub newly_retired: Vec<u64>,
    /// Every retired generation, cumulative and sorted. Retirement is
    /// permanent: a retired generation's unique hashes are unreachable,
    /// and unreachable code cannot come back into reach.
    pub retired: Vec<u64>,
    /// Old generations still live, with what pins them.
    pub pinned: Vec<PinnedGeneration>,
    /// Every hash the trace reached — the safety roots for purging the
    /// on-disk store while a system is live (`DiskStore::gc` extra
    /// roots).
    pub reachable: Vec<blake3::Hash>,
}

/// A name the current deploy changed: rebound (same signature) or
/// retired-and-fresh (signature changed). Input to the deploy warnings.
pub(crate) struct ChangedName {
    pub(crate) name: Arc<str>,
    pub(crate) old: blake3::Hash,
    pub(crate) new: blake3::Hash,
}

/// A deploy-report warning (see `ref/live-upgrade.md`, "Deploy
/// diagnostics"): a failure mode content addressing lets the deploy pass
/// *see* rather than hit silently.
#[derive(Debug, Clone)]
pub enum DeployWarning {
    /// A changed name whose running old copy can never pick the change
    /// up — it can only retire on restart (or when its holder drops it).
    UnreachableChange {
        name: Arc<str>,
        /// The root holding the old copy.
        pinned_by: RootOrigin,
        /// True when the name's lineage is severed (`latest` resolves
        /// the old hash to itself — a signature change or diverged
        /// aliases), so the holder's own late binding can never go
        /// forward. False when the change is orphaned: no live re-entry
        /// point (the entry, a task body resolution) reaches the new
        /// code at all.
        severed: bool,
    },
    /// Live (old-generation) code performs an ability method key that no
    /// current handler covers, while current code *does* handle that
    /// ability — the ability-evolution drift channel: the perform falls
    /// through to its old default, soundly but silently.
    UncoveredMethodKey {
        ability: uuid::Uuid,
        method: ambient_core::MethodKey,
        /// What performs it (deployed name, named ancestor, or hash).
        performer: String,
    },
}

impl std::fmt::Display for DeployWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnreachableChange {
                name,
                pinned_by,
                severed: true,
            } => write!(
                f,
                "`{name}` changed, but the running copy (pinned by {pinned_by}) resolves to \
                 itself — the change can only land on restart"
            ),
            Self::UnreachableChange {
                name,
                pinned_by,
                severed: false,
            } => write!(
                f,
                "`{name}` changed, but no live re-entry point reaches the new code — the \
                 running copy (pinned by {pinned_by}) can only retire on restart"
            ),
            Self::UncoveredMethodKey {
                ability,
                method,
                performer,
            } => write!(
                f,
                "live code in {performer} performs method {} of ability {ability}, which no \
                 current handler covers — the perform falls through to its old default",
                method.short_hex()
            ),
        }
    }
}

/// The generation ledger: which deploy shipped which hashes, what names
/// they carried, and which generations have retired. Owned by the deploy
/// runtime, updated on every successful swap.
#[derive(Default)]
pub(crate) struct Ledger {
    /// Number of generations recorded; ids are 1-based and dense, so
    /// this is also the current generation's id (0 = none yet).
    generations: u64,
    /// Hash → the most recent generation that shipped it (attribution
    /// for retirement — see the module docs).
    latest_shipper: HashMap<blake3::Hash, u64>,
    /// Hash → diagnostic label (deployed name, or named ancestor).
    labels: HashMap<blake3::Hash, Label>,
    /// Retired generation ids (sticky).
    retired: BTreeSet<u64>,
}

impl Ledger {
    /// Record a successfully validated deploy: assign the next
    /// generation id, attribute every shipped hash to it, and refresh
    /// labels from its name bindings.
    pub(crate) fn record(&mut self, generation: &Generation) -> u64 {
        let id = self.generations + 1;
        let shipped = generation
            .functions
            .keys()
            .chain(generation.values.keys())
            .chain(generation.natives.keys());
        for hash in shipped {
            self.latest_shipper.insert(*hash, id);
        }
        for (name, binding) in &generation.bindings {
            self.labels.insert(
                binding.hash,
                Label {
                    name: Arc::clone(name),
                    direct: true,
                },
            );
        }
        // Propagate names down to anonymous hashes (lambdas, consts): an
        // unlabeled dependency inherits its labeled parent's base name.
        // Iterate to a fixpoint so chains of lambdas resolve to the
        // innermost *named* ancestor.
        loop {
            let mut inherited = Vec::new();
            for func in generation.functions.values() {
                let Some(label) = self.labels.get(&func.hash) else {
                    continue;
                };
                let base = Arc::clone(&label.name);
                for dep in &func.dependencies {
                    if !self.labels.contains_key(dep) {
                        inherited.push((*dep, Arc::clone(&base)));
                    }
                }
            }
            if inherited.is_empty() {
                break;
            }
            for (hash, name) in inherited {
                self.labels.entry(hash).or_insert(Label {
                    name,
                    direct: false,
                });
            }
        }
        self.generations = id;
        id
    }

    /// The diagnostic label for a hash, if any.
    fn label(&self, hash: &blake3::Hash) -> Option<Label> {
        self.labels.get(hash).cloned()
    }

    /// Classify every non-current, non-retired generation against a
    /// trace: generations with no reachable latest-shipped hash retire
    /// (permanently); the rest report their pins.
    pub(crate) fn classify(&mut self, reach: &Reach, origins: &[RootOrigin]) -> RetirementReport {
        let current = (self.generations > 0).then_some(self.generations);

        // Group the reachable hashes by attributed generation.
        let mut pinned_hashes: BTreeMap<u64, Vec<(blake3::Hash, usize)>> = BTreeMap::new();
        for (hash, root) in &reach.seen {
            if let Some(id) = self.latest_shipper.get(hash) {
                pinned_hashes.entry(*id).or_default().push((*hash, *root));
            }
        }

        let mut report = RetirementReport {
            current,
            ..RetirementReport::default()
        };
        for id in 1..=self.generations {
            if Some(id) == current || self.retired.contains(&id) {
                continue;
            }
            match pinned_hashes.get(&id) {
                None => {
                    self.retired.insert(id);
                    report.newly_retired.push(id);
                }
                Some(pins) => {
                    let mut pins: Vec<Pin> = pins
                        .iter()
                        .map(|(hash, root)| Pin {
                            root: origins[*root].clone(),
                            hash: *hash,
                            label: self.label(hash),
                        })
                        .collect();
                    // Directly named pins first, then labeled lambdas,
                    // then by hash for determinism — the first pin is
                    // what the dev loop prints.
                    pins.sort_by_key(|pin| {
                        let direct = pin.label.as_ref().is_some_and(|label| label.direct);
                        (pin.label.is_none(), !direct, pin.hash.to_hex())
                    });
                    report.pinned.push(PinnedGeneration { id, pins });
                }
            }
        }
        report.retired = self.retired.iter().copied().collect();
        report.reachable = reach.seen.keys().copied().collect();
        report.reachable.sort_unstable_by_key(blake3::Hash::to_hex);
        report
    }

    /// Compute one deploy's warnings (see [`DeployWarning`]).
    ///
    /// - `live` is the reach from runtime-held roots only (cells and
    ///   the task registry) — the code the running system holds.
    /// - `fresh` is the reach from live re-entry points: the entry (it
    ///   re-runs on every deploy) plus every task root's forward
    ///   resolution (the runtime re-resolves task bodies each pass).
    /// - `changed` is the swap's rebound + retired names.
    /// - `latest` is one forward resolution against the swapped table.
    pub(crate) fn warnings(
        &self,
        loaded: &Loaded,
        live: &Reach,
        fresh: &Reach,
        origins: &[RootOrigin],
        changed: &[ChangedName],
        latest: &dyn Fn(&blake3::Hash) -> blake3::Hash,
    ) -> Vec<DeployWarning> {
        let mut warnings = Vec::new();

        // Unreachable change: a changed name with a live old copy that
        // the change can never reach — its lineage is severed (the
        // holder's resolution returns the old hash forever), or the new
        // code is orphaned from every live re-entry point.
        let mut changed: Vec<&ChangedName> = changed.iter().collect();
        changed.sort_by_key(|change| Arc::clone(&change.name));
        for change in changed {
            let Some(root) = live.seen.get(&change.old) else {
                continue;
            };
            let severed = latest(&change.old) == change.old;
            if severed || !fresh.seen.contains_key(&change.new) {
                warnings.push(DeployWarning::UnreachableChange {
                    name: Arc::clone(&change.name),
                    pinned_by: origins[*root].clone(),
                    severed,
                });
            }
        }

        // Uncovered method key: strictly-old live code performs a key
        // that current code neither performs nor covers, while current
        // code *does* cover some key of the same ability — the method
        // was re-keyed (ability evolution) under the old code's feet.
        // The ability scoping is deliberate: default-implemented
        // abilities that are never handled in bytecode (State, Time,
        // ...) must not warn on every old perform.
        let mut covered: HashSet<ambient_core::MethodKey> = HashSet::new();
        let mut fresh_performed: HashSet<ambient_core::MethodKey> = HashSet::new();
        let mut fresh_covered_abilities: HashSet<uuid::Uuid> = HashSet::new();
        for hash in live.seen.keys().chain(fresh.seen.keys()) {
            let Some(func) = loaded.functions.get(hash) else {
                continue;
            };
            let is_fresh = fresh.seen.contains_key(hash);
            let sites = ambient_engine::bytecode::method_ref_sites(func);
            for method in sites.covered {
                covered.insert(method.method_key());
                if is_fresh {
                    fresh_covered_abilities.insert(method.ability_uuid);
                }
            }
            if is_fresh {
                for method in sites.performed {
                    fresh_performed.insert(method.method_key());
                }
            }
        }
        let mut old_code: Vec<&blake3::Hash> = live
            .seen
            .keys()
            .filter(|hash| !fresh.seen.contains_key(*hash))
            .collect();
        old_code.sort_unstable_by_key(|hash| hash.to_hex());
        let mut warned: HashSet<ambient_core::MethodKey> = HashSet::new();
        for hash in old_code {
            let Some(func) = loaded.functions.get(hash) else {
                continue;
            };
            for method in ambient_engine::bytecode::method_ref_sites(func).performed {
                let key = method.method_key();
                if covered.contains(&key)
                    || fresh_performed.contains(&key)
                    || !fresh_covered_abilities.contains(&method.ability_uuid)
                    || !warned.insert(key)
                {
                    continue;
                }
                warnings.push(DeployWarning::UncoveredMethodKey {
                    ability: method.ability_uuid,
                    method: key,
                    performer: describe_hash(self.labels.get(hash), hash),
                });
            }
        }
        warnings
    }
}
