//! Import-cycle detection over the module dependency graph.
//!
//! The module dependency graph is a hard DAG: an import cycle *between*
//! modules is a compile error (Go's rule). Recursion stays **within** a
//! module — a function calling itself or a sibling, or a `use self::…` of
//! the module's own items, is a same-module reference and never a
//! dependency edge — so only genuine cross-module cycles are rejected here.
//!
//! This is the single *decision* of what counts as an import cycle, in the
//! engine, invoked by both frontends so they can never disagree:
//!
//! - [`build_package`](crate::build::build_package) calls
//!   [`detect_import_cycles`] on the dependency map it already assembles and
//!   fails the build ([`BuildError::ImportCycle`](crate::build::BuildError)).
//! - The analysis pipeline (`ambient check` and the LSP, both through
//!   `ambient_analysis::analyze_with_registry`) calls
//!   [`import_cycle_containing`] and reports the same rendered text as a
//!   per-module diagnostic.
//!
//! Both render through [`ImportCycle::describe`], so `ambient run`,
//! `ambient check`, and the editor produce byte-identical cycle text.
//!
//! Detection is deliberately scoped to a single package's modules: core and
//! platform module groups are authored cycle-free and compiled by
//! `compile_module_group`'s own topo sort, and no `core`/`platform` module
//! can import a user module, so a user-package cycle can never route through
//! them. [`import_cycle_containing`] restricts the graph to modules sharing
//! the queried module's [`Scope`](crate::fqn::Scope) for exactly this reason.

use std::collections::BTreeMap;

use crate::fqn::ModuleId;
use crate::module_path::ModulePath;
use crate::module_registry::ModuleRegistry;

/// A detected import cycle among a package's modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCycle {
    /// The modules forming the cycle, in traversal order and rotated to
    /// begin at the lexically-least module (`["a", "b"]` for `a -> b -> a`).
    /// Keys are dotted module paths (`a`, `net::http`) — the same
    /// [`ModuleId::module_path_string`] form both frontends key their graphs
    /// on. Canonical regardless of the input map's iteration order.
    modules: Vec<String>,
}

impl ImportCycle {
    /// The module keys participating in the cycle.
    #[must_use]
    pub fn members(&self) -> &[String] {
        &self.modules
    }

    /// The human-readable cycle, e.g. `import cycle: pkg::a -> pkg::b ->
    /// pkg::a`. The path is rooted with the literal `pkg` package-root
    /// keyword (not the package name), so it is identical no matter which
    /// package the cycle lives in — and the loop is closed by repeating the
    /// first module at the end. This is the *single* rendering both the
    /// build error and the analysis diagnostic use.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut rendered: Vec<String> = self.modules.iter().map(|m| format!("pkg::{m}")).collect();
        if let Some(first) = self.modules.first() {
            rendered.push(format!("pkg::{first}"));
        }
        format!("import cycle: {}", rendered.join(" -> "))
    }
}

/// Detect every import cycle in a module dependency graph.
///
/// `deps` maps each package module (by [`ModuleId::module_path_string`]) to
/// the modules it depends on. Only edges whose target is itself a key
/// participate — values naming `core`/`platform` (or any non-key) modules
/// are ignored, since they can never close a cycle back into the package.
///
/// Returns one canonical cycle per cyclic strongly-connected component,
/// sorted so the result is deterministic (the lexically-least cycle first).
#[must_use]
pub fn detect_import_cycles(deps: &BTreeMap<String, Vec<String>>) -> Vec<ImportCycle> {
    let graph = Graph::new(deps);
    let mut cycles: Vec<ImportCycle> = graph
        .strongly_connected_components()
        .into_iter()
        .filter_map(|scc| graph.canonical_cycle(&scc))
        .collect();
    cycles.sort_by(|a, b| a.modules.cmp(&b.modules));
    cycles
}

/// The import cycle the module at `module_path` participates in, if any.
///
/// Reconstructs the package's dependency graph from the registry's
/// (resolved) module ASTs — re-running the resolve pass on each is
/// idempotent and yields the same dependency set the build orders on — then
/// finds the cycle containing this module. Restricted to modules sharing
/// this module's [`Scope`](crate::fqn::Scope): core and platform groups are
/// separately guaranteed acyclic and cannot import user code, so a package
/// cycle is always wholly within the package's own scope.
///
/// This is what the analysis pipeline calls per module; every module in a
/// cycle independently finds the same canonical cycle, so `ambient check`
/// and the LSP report identical text at each participating file.
#[must_use]
pub fn import_cycle_containing(
    registry: &ModuleRegistry,
    module_path: &ModulePath,
) -> Option<ImportCycle> {
    let current_id = registry.module_id(module_path);
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for info in registry.all_modules() {
        let module_id = registry.module_id(&info.path);
        // A package cycle stays within one scope; skip core/platform and any
        // other scope so re-resolving the whole standard library on every
        // check is avoided.
        if module_id.scope != current_id.scope {
            continue;
        }
        let mut ast = (*info.module).clone();
        let outcome = crate::resolve::resolve_module(&mut ast, &info.path, registry);
        let edges = outcome
            .deps
            .iter()
            .filter(|dep| dep.scope == current_id.scope)
            .map(ModuleId::module_path_string)
            .collect();
        deps.insert(module_id.module_path_string(), edges);
    }

    let current_key = current_id.module_path_string();
    detect_import_cycles(&deps)
        .into_iter()
        .find(|cycle| cycle.members().contains(&current_key))
}

/// Map every module that participates in a cycle to the canonical cycle it is
/// in, from a prebuilt dependency graph (`module_path_string` keys, edges to
/// same-package modules only — the same shape [`detect_import_cycles`] takes).
///
/// This is the batch form the incremental analysis session uses: it computes
/// the whole package's cycle set **once per registry revision** from dependency
/// edges it already has (no per-module re-resolve), replacing the O(modules²)
/// [`import_cycle_containing`] loop. A module absent from the result is in no
/// cycle. Every member of one cycle maps to the *same* [`ImportCycle`], so each
/// participating file renders byte-identical text — exactly as
/// [`import_cycle_containing`] would report it per module.
#[must_use]
pub fn cycles_by_member(deps: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, ImportCycle> {
    let mut out = BTreeMap::new();
    for cycle in detect_import_cycles(deps) {
        for member in cycle.members() {
            out.insert(member.clone(), cycle.clone());
        }
    }
    out
}

/// A directed graph over module keys, indexed by dense node ids for the SCC
/// pass. Only intra-graph edges (targets that are themselves nodes) are kept.
struct Graph {
    /// Node id → module key, in sorted order (index 0 is the lexically-least
    /// key), so every traversal that visits nodes/neighbors in id order is
    /// deterministic.
    keys: Vec<String>,
    /// Adjacency by node id, each neighbor list ascending.
    adjacency: Vec<Vec<usize>>,
}

impl Graph {
    fn new(deps: &BTreeMap<String, Vec<String>>) -> Self {
        // `deps` is a `BTreeMap`, so its keys are already sorted; assign
        // dense ids in that order.
        let keys: Vec<String> = deps.keys().cloned().collect();
        let index: BTreeMap<&str, usize> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let adjacency = keys
            .iter()
            .map(|key| {
                let mut targets: Vec<usize> = deps[key]
                    .iter()
                    .filter_map(|dep| index.get(dep.as_str()).copied())
                    .collect();
                targets.sort_unstable();
                targets.dedup();
                targets
            })
            .collect();
        Self { keys, adjacency }
    }

    /// Tarjan's strongly-connected-components, returned as node-id groups.
    /// Iterating roots and neighbors in ascending id order keeps the output
    /// deterministic.
    fn strongly_connected_components(&self) -> Vec<Vec<usize>> {
        let n = self.keys.len();
        let mut state = Tarjan {
            adjacency: &self.adjacency,
            index: vec![usize::MAX; n],
            lowlink: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::new(),
            next_index: 0,
            components: Vec::new(),
        };
        for node in 0..n {
            if state.index[node] == usize::MAX {
                state.visit(node);
            }
        }
        state.components
    }

    /// The canonical simple cycle for a strongly-connected component, or
    /// `None` if the component is a single node with no self-edge (not a
    /// cycle). The cycle starts at the component's lexically-least node and
    /// is the shortest cycle through it (BFS, neighbors visited in ascending
    /// order — deterministic).
    fn canonical_cycle(&self, scc: &[usize]) -> Option<ImportCycle> {
        // A lone node is cyclic only through a self-edge.
        if scc.len() == 1 {
            let node = scc[0];
            return self.adjacency[node].contains(&node).then(|| ImportCycle {
                modules: vec![self.keys[node].clone()],
            });
        }

        let in_scc: Vec<bool> = {
            let mut flags = vec![false; self.keys.len()];
            for &node in scc {
                flags[node] = true;
            }
            flags
        };
        // The SCC's lexically-least node is its minimum id (ids follow the
        // sorted key order).
        let start = *scc.iter().min()?;

        // BFS for the shortest path from `start` back to `start`, staying in
        // the component. Seed with `start`'s neighbors so the trivial
        // zero-length "path" doesn't stop the search immediately.
        let mut predecessor: BTreeMap<usize, usize> = BTreeMap::new();
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for &neighbor in &self.adjacency[start] {
            if in_scc[neighbor] && !predecessor.contains_key(&neighbor) {
                predecessor.insert(neighbor, start);
                queue.push_back(neighbor);
            }
        }
        while let Some(node) = queue.pop_front() {
            for &neighbor in &self.adjacency[node] {
                if neighbor == start {
                    return Some(self.reconstruct(start, node, &predecessor));
                }
                if in_scc[neighbor] && !predecessor.contains_key(&neighbor) {
                    predecessor.insert(neighbor, node);
                    queue.push_back(neighbor);
                }
            }
        }
        None
    }

    /// Rebuild `start -> … -> end` from BFS predecessors into an
    /// `ImportCycle` (the closing edge `end -> start` is implied and rendered
    /// by [`ImportCycle::describe`]).
    fn reconstruct(
        &self,
        start: usize,
        end: usize,
        predecessor: &BTreeMap<usize, usize>,
    ) -> ImportCycle {
        let mut path = vec![end];
        let mut node = end;
        while node != start {
            node = predecessor[&node];
            path.push(node);
        }
        path.reverse();
        ImportCycle {
            modules: path.into_iter().map(|id| self.keys[id].clone()).collect(),
        }
    }
}

/// Mutable state for one Tarjan SCC traversal.
struct Tarjan<'a> {
    adjacency: &'a [Vec<usize>],
    index: Vec<usize>,
    lowlink: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next_index: usize,
    components: Vec<Vec<usize>>,
}

impl Tarjan<'_> {
    fn visit(&mut self, node: usize) {
        self.index[node] = self.next_index;
        self.lowlink[node] = self.next_index;
        self.next_index += 1;
        self.stack.push(node);
        self.on_stack[node] = true;

        for i in 0..self.adjacency[node].len() {
            let neighbor = self.adjacency[node][i];
            if self.index[neighbor] == usize::MAX {
                self.visit(neighbor);
                self.lowlink[node] = self.lowlink[node].min(self.lowlink[neighbor]);
            } else if self.on_stack[neighbor] {
                self.lowlink[node] = self.lowlink[node].min(self.index[neighbor]);
            }
        }

        if self.lowlink[node] == self.index[node] {
            let mut component = Vec::new();
            loop {
                let popped = self.stack.pop().unwrap_or(node);
                self.on_stack[popped] = false;
                component.push(popped);
                if popped == node {
                    break;
                }
            }
            component.sort_unstable();
            self.components.push(component);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(edges: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(k, vs)| {
                (
                    (*k).to_string(),
                    vs.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn acyclic_graph_has_no_cycles() {
        let deps = graph(&[("main", &["a", "b"]), ("a", &["b"]), ("b", &[])]);
        assert!(detect_import_cycles(&deps).is_empty());
    }

    #[test]
    fn non_key_targets_are_ignored() {
        // `a` depends on `core::primitives`, which is not a node: no cycle.
        let deps = graph(&[("a", &["core::primitives"]), ("b", &[])]);
        assert!(detect_import_cycles(&deps).is_empty());
    }

    #[test]
    fn two_module_cycle_is_canonicalized() {
        // Same cycle described from either direction rotates to the same
        // lexically-least start.
        let ab = graph(&[("a", &["b"]), ("b", &["a"])]);
        let ba = graph(&[("b", &["a"]), ("a", &["b"])]);
        let one = detect_import_cycles(&ab);
        let two = detect_import_cycles(&ba);
        assert_eq!(one.len(), 1);
        assert_eq!(one, two);
        assert_eq!(one[0].members(), &["a".to_string(), "b".to_string()]);
        assert_eq!(
            one[0].describe(),
            "import cycle: pkg::a -> pkg::b -> pkg::a"
        );
    }

    #[test]
    fn three_module_cycle_rotates_to_least() {
        // b -> c -> a -> b, however spelled, reports starting at `a`.
        let deps = graph(&[("b", &["c"]), ("c", &["a"]), ("a", &["b"])]);
        let cycles = detect_import_cycles(&deps);
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0].members(),
            &["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(
            cycles[0].describe(),
            "import cycle: pkg::a -> pkg::b -> pkg::c -> pkg::a"
        );
    }

    #[test]
    fn self_edge_is_a_cycle() {
        let deps = graph(&[("a", &["a"])]);
        let cycles = detect_import_cycles(&deps);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].describe(), "import cycle: pkg::a -> pkg::a");
    }

    #[test]
    fn shortest_cycle_through_least_node_is_reported() {
        // One SCC {a,b,c} with two cycles through `a`: a->b->a (len 2) and
        // a->c->b->a is not present; here a->b->a is shortest.
        let deps = graph(&[("a", &["b", "c"]), ("b", &["a"]), ("c", &["b"])]);
        let cycles = detect_import_cycles(&deps);
        assert_eq!(cycles.len(), 1);
        // Shortest cycle through `a` is a -> b -> a.
        assert_eq!(cycles[0].members(), &["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn cycles_by_member_maps_every_participant_to_its_cycle() {
        // Two disjoint cycles plus an acyclic module: each cyclic member maps
        // to its own cycle; the acyclic module is absent.
        let deps = graph(&[
            ("a", &["b"]),
            ("b", &["a"]),
            ("x", &["y"]),
            ("y", &["x"]),
            ("free", &["a"]),
        ]);
        let by_member = cycles_by_member(&deps);
        assert_eq!(by_member.len(), 4);
        assert_eq!(by_member["a"], by_member["b"]);
        assert_eq!(by_member["x"], by_member["y"]);
        assert_ne!(by_member["a"], by_member["x"]);
        assert!(!by_member.contains_key("free"));
        // The mapped cycle is the same one `detect_import_cycles` reports.
        assert_eq!(
            by_member["a"].members(),
            &["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn disjoint_cycles_are_both_reported_sorted() {
        let deps = graph(&[("a", &["b"]), ("b", &["a"]), ("x", &["y"]), ("y", &["x"])]);
        let cycles = detect_import_cycles(&deps);
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0].members()[0], "a");
        assert_eq!(cycles[1].members()[0], "x");
    }
}
