//! Lexical environment and evaluator value domain.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use tlib::{TreeArena, TreeId};

use crate::source_context::EvalSourceContext;
use crate::{EnvId, SymId};

/// Evaluator value domain used during Phase 4.
///
/// Rust keeps closures and pattern matchers as explicit evaluator values rather
/// than as tree-encoded host nodes, then lowers residual values back to boxes.
#[derive(Clone, Debug)]
pub(crate) enum EvalValue {
    Box(TreeId),
    Closure(ClosureValue),
    PatternMatcher(PatternMatcherValue),
}

impl EvalValue {
    /// Returns the tree node used to display or lower this value back to a box.
    ///
    /// For a `Box` this is the node itself; for closures and pattern matchers it
    /// is the underlying expression / case-expression node.
    pub(crate) fn display_tree(&self) -> TreeId {
        match self {
            Self::Box(id) => *id,
            Self::Closure(closure) => closure.expr,
            Self::PatternMatcher(pm) => pm.case_expr,
        }
    }
}

impl PartialEq for EvalValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Box(lhs), Self::Box(rhs)) => lhs == rhs,
            (Self::Closure(lhs), Self::Closure(rhs)) => lhs == rhs,
            (Self::PatternMatcher(lhs), Self::PatternMatcher(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

impl Eq for EvalValue {}

#[derive(Clone, Debug)]
/// Captured closure value for delayed evaluation in one lexical environment.
pub(crate) struct ClosureValue {
    pub(crate) expr: TreeId,
    pub(crate) env: Environment,
}

impl PartialEq for ClosureValue {
    fn eq(&self, other: &Self) -> bool {
        self.expr == other.expr && self.env.same_identity(&other.env)
    }
}

impl Eq for ClosureValue {}

#[derive(Clone, Debug)]
/// Captured pattern-matcher automaton value used by residual `case` handling.
pub(crate) struct PatternMatcherValue {
    pub(crate) automaton: crate::pattern_matcher::Automaton,
    pub(crate) state: usize,
    pub(crate) envs: Vec<Option<Environment>>,
    pub(crate) original_rules: TreeId,
    pub(crate) rev_param_list: Vec<TreeId>,
    pub(crate) case_expr: TreeId,
}

impl PartialEq for PatternMatcherValue {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
            && self.original_rules == other.original_rules
            && self.rev_param_list == other.rev_param_list
            && self.case_expr == other.case_expr
            && self.automaton.rhs == other.automaton.rhs
            && self
                .envs
                .iter()
                .zip(other.envs.iter())
                .all(|(lhs, rhs)| match (lhs, rhs) {
                    (Some(lhs), Some(rhs)) => lhs.same_identity(rhs),
                    (None, None) => true,
                    _ => false,
                })
            && self.envs.len() == other.envs.len()
    }
}

impl Eq for PatternMatcherValue {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Stable environment identity paired with one symbol for recursion tracking.
pub(crate) struct EnvFrameKey {
    pub(crate) store_ptr: usize,
    pub(crate) env_id: EnvId,
    pub(crate) source_context_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Memoization key for `eval(expr, env)` parity with C++ `getEvalProperty`.
pub(crate) struct EvalCacheKey {
    pub(crate) expr: TreeId,
    pub(crate) env_key: EnvFrameKey,
}

#[derive(Clone, Debug)]
/// One lexical environment layer.
pub(crate) struct EnvLayer {
    bindings: Vec<(SymId, EvalValue)>,
    binding_index: ahash::HashMap<SymId, usize>,
    parent: Option<EnvId>,
    barrier: bool,
}

impl Default for EnvLayer {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
            binding_index: ahash::HashMap::with_hasher(ahash::RandomState::new()),
            parent: None,
            barrier: false,
        }
    }
}

#[derive(Debug, Default)]
/// Arena of lexical environment layers.
pub(crate) struct EnvStore {
    pub(crate) layers: Vec<EnvLayer>,
}

/// Lexical evaluation environment mapping symbol names to box-IR tree nodes.
///
/// # Architecture
///
/// Implemented as a **linked list of scopes**, where each scope is a `Vec<(SymId, TreeId)>`:
///
/// ```text
/// Environment { bindings: [("f", 42)], parent: Some(→) }
///      │ parent
/// Environment { bindings: [("x", 17), ("y", 8)], parent: Some(→) }
///      │ parent
/// Environment { bindings: [("process", 3)], parent: None }  ← root scope
/// ```
///
/// Lookup walks from innermost to outermost scope, returning the first match (shadowing semantics).
/// Binding always targets the **current** scope, never a parent.
///
/// # C++ correspondence — `environment.cpp`
///
/// The C++ environment is a **persistent linked list of `ENV_LAYER` tree nodes** stored in the
/// global hash-cons pool. Key differences from this Rust implementation:
///
/// | Aspect | C++ (`environment.cpp`) | Rust (`Environment`) |
/// |---|---|---|
/// | Storage | Hash-consed tree nodes with property tables | `Vec<(u32, TreeId)>` — interned `SymId` |
/// | Values stored | **Closures**: `closure(expr, genv, visited, lenv)` capturing the scope at definition time | `EvalValue::{Box, Closure}` in the current adapted model |
/// | Lookup | `searchIdDef`: walks layers calling `getProperty` (hash probe per layer) | `lookup`: per-layer symbol index lookup, then recurse to `parent` |
/// | Scope push | `pushNewLayer(lenv)` — allocates a unique tree node | `push_scope()` — allocate one arena layer and return its `EnvId` handle |
/// | Redefinition | `addLayerDef` throws `faustexception` on conflicting rebind | `bind_definitions` returns `EvalError::RedefinedSymbol` |
/// | Barrier | `pushEnvBarrier` / `isEnvBarrier` — stops pattern-matcher lookup | `push_barrier_scope()` / `lookup_until_barrier()` |
/// | Env copy/rewire | `copyEnvReplaceDefs` + `updateClosures` — for captured-env rewrites | Deferred in the current Rust model |
/// | Profiling | `gGlobal->gStats.fEnvLayersPushed/fEnvLookups/fEnvLookupTotalDepth` | [`EvalStats`](crate::EvalStats) returned by [`eval`](crate::eval) |
///
/// # Performance
///
/// For typical Faust programs (scope depth ≤ 5, ≤ 30 bindings/scope):
/// - **Lookup**: O(d) hash probes where d = scope depth. Bindings remain stored
///   in insertion order for diagnostics/snapshots, while `binding_index` tracks
///   the latest binding for each symbol in one layer.
/// - **Bind**: `Vec::push` — amortized O(1), no hashing, no pointer chasing.
/// - **Push scope**: O(1) one-layer allocation in the shared environment arena.
/// - **Memory per binding**: one inline `(SymId, EvalValue)` pair in the current layer vector.
///   The frequently-hit plain-box case stores only one symbol id plus one small tagged payload and
///   avoids per-binding heap allocation; closure and pattern-matcher bindings carry larger inline
///   state. The earlier `8 bytes` rule of thumb applies only to the narrow `SymId + TreeId` box
///   payload shape and should not be read as the size of every binding variant.
#[derive(Clone, Debug)]
pub struct Environment {
    pub(crate) store: Rc<RefCell<EnvStore>>,
    pub(crate) current: EnvId,
    pub(crate) source_context: Arc<EvalSourceContext>,
}

impl Environment {
    /// Creates an empty root environment with no bindings and no parent.
    ///
    /// **C++ equivalent**: the initial `gGlobal->nil` environment passed to the first
    /// `pushMultiClosureDefs` call in `eval.cpp`.
    #[must_use]
    pub fn empty() -> Self {
        Self::empty_with_source_context(EvalSourceContext::memory())
    }

    /// Creates an empty root environment with one captured source-resolution context.
    ///
    /// This is the Rust equivalent of initializing evaluation under one configured
    /// `gReader` session in the C++ compiler. All child scopes and closures derived
    /// from this environment inherit the same context unless a newly loaded file
    /// installs a more specific one.
    ///
    /// Typical callers:
    /// - [`eval`](crate::eval) with [`EvalRequest::with_source_context`](crate::EvalRequest::with_source_context) for file-backed compilation,
    /// - targeted tests exercising `component("...")` / `library("...")` parity.
    #[must_use]
    pub fn empty_with_source_context(source_context: EvalSourceContext) -> Self {
        let mut store = EnvStore::default();
        store.layers.push(EnvLayer::default());
        Self {
            store: Rc::new(RefCell::new(store)),
            current: 0,
            source_context: Arc::new(source_context),
        }
    }

    /// Returns the stable id of the current environment layer.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/environment.cpp`
    /// - `pushNewLayer`
    ///
    /// In C++, environment identity is carried by the `Tree` pointer of the `ENV_LAYER` node.
    /// In Rust it is carried by this dense arena index.
    #[must_use]
    pub fn id(&self) -> EnvId {
        self.current
    }

    /// Returns `true` if both handles refer to the same layer in the same arena.
    ///
    /// Compares the current layer id together with the identity of the shared
    /// store and source context (pointer equality), so two clones of the same
    /// environment compare equal while structurally identical but distinct
    /// environments do not.
    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.current == other.current
            && Rc::ptr_eq(&self.store, &other.store)
            && Arc::ptr_eq(&self.source_context, &other.source_context)
    }

    /// Returns a globally unique key identifying this environment's current layer.
    ///
    /// Used as a memoization/loop-detection key (see [`crate::loop_detector`]).
    pub(crate) fn frame_key(&self) -> EnvFrameKey {
        self.frame_key_for(self.current)
    }

    /// Builds an [`EnvFrameKey`] for an arbitrary layer of this environment's store.
    ///
    /// Combines the store and source-context pointers with `env_id` so keys are
    /// unique across distinct arenas as well as across layers.
    pub(crate) fn frame_key_for(&self, env_id: EnvId) -> EnvFrameKey {
        EnvFrameKey {
            store_ptr: Rc::as_ptr(&self.store) as usize,
            env_id,
            source_context_ptr: Arc::as_ptr(&self.source_context) as usize,
        }
    }

    /// Returns the source-resolution context captured by this environment.
    ///
    /// This is the context inherited by closures created while evaluating in
    /// this environment. For file-backed sessions it identifies the file and
    /// import-search roots that `component("...")` / `library("...")` will use.
    #[must_use]
    pub fn source_context(&self) -> &EvalSourceContext {
        self.source_context.as_ref()
    }

    /// Binds one symbol to a tree node in the **current scope** (not a parent).
    ///
    /// This is the unchecked low-level binder. Multiple bindings for the same name in the same
    /// scope are allowed (last binding wins on lookup — shadowing). For definitions that must
    /// enforce the no-redefinition rule, use `bind_definitions` which calls
    /// [`lookup_local`](Self::lookup_local) and returns `EvalError::RedefinedSymbol` on conflict.
    ///
    /// **C++ equivalent**: `setProperty(lenv, id, def)` in `addLayerDef`, but without the
    /// prior duplicate check. The check is performed externally in `bind_definitions`.
    ///
    /// `sym` must be obtained from [`TreeArena::intern_symbol`] before calling this method.
    pub fn bind(&mut self, sym: SymId, value: TreeId) {
        self.bind_value(sym, EvalValue::Box(value));
    }

    /// Binds a symbol to an arbitrary [`EvalValue`] in the current scope.
    ///
    /// The general form behind [`bind`](Self::bind): it accepts closures and
    /// pattern matchers in addition to plain boxes. Like `bind`, it is unchecked
    /// — duplicate bindings shadow rather than error.
    pub(crate) fn bind_value(&mut self, sym: SymId, value: EvalValue) {
        self.with_store_mut(|store| {
            let layer = &mut store.layers[self.current];
            let index = layer.bindings.len();
            layer.bindings.push((sym, value));
            layer.binding_index.insert(sym, index);
        });
    }

    /// Looks up a symbol across the full scope chain (current scope and all parents).
    ///
    /// Returns the **innermost** (most recently bound) binding for `sym`, or `None` if the
    /// symbol is not visible in any scope. This implements shadowing: a binding in an inner scope
    /// hides any binding with the same `SymId` in an outer scope.
    ///
    /// The scan is performed in **reverse insertion order** within each scope (O(n) integer
    /// comparisons), then recurses to the parent. The last `bind` call for a given `sym` in the
    /// current scope wins. Comparison is O(1) — two `u32` values compared by equality.
    ///
    /// **C++ equivalent**: `getProperty(lenv, id, def)` called in a loop walking the layer chain
    /// (`lenv = lenv->branch(0)`) until a hit or `isEnvBarrier`. The C++ lookup uses a hash
    /// probe per layer; the Rust lookup uses a linear SIMD-friendly `u32` scan per layer.
    ///
    /// **Does not stop at barriers**: unlike C++ `searchIdDef` (used by the pattern matcher),
    /// this lookup traverses the full parent chain. Pattern-matching nonlinearity checks must
    /// instead use [`lookup_until_barrier`](Self::lookup_until_barrier).
    ///
    /// `sym` must be a valid id returned by [`TreeArena::intern_symbol`] or
    /// [`TreeArena::get_symbol`].
    #[must_use]
    pub fn lookup(&self, sym: SymId) -> Option<TreeId> {
        self.lookup_value(sym).and_then(|(_, value)| match value {
            EvalValue::Box(id) => Some(id),
            EvalValue::Closure(_) => None,
            EvalValue::PatternMatcher(_) => None,
        })
    }

    /// Value-returning core of [`lookup`](Self::lookup).
    ///
    /// Walks the full scope chain and returns the innermost binding as an
    /// [`EvalValue`] together with the [`EnvId`] of the layer that owns it, so
    /// callers that care about closures/matchers (not just boxes) can use them.
    pub(crate) fn lookup_value(&self, sym: SymId) -> Option<(EnvId, EvalValue)> {
        self.with_store(|store| {
            let mut env_id = Some(self.current);
            while let Some(id) = env_id {
                let layer = &store.layers[id];
                if let Some(&index) = layer.binding_index.get(&sym) {
                    return Some((id, layer.bindings[index].1.clone()));
                }
                env_id = layer.parent;
            }
            None
        })
    }

    /// Looks up a symbol across the current scope chain but stops when a barrier scope is reached.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/environment.cpp`
    /// - `searchIdDef`
    /// - `pushEnvBarrier`
    ///
    /// This is used by the pattern matcher to implement rule-local nonlinearity:
    /// repeated pattern variables must only compare against bindings created while matching the
    /// current rule. Outer lexical bindings remain visible to normal RHS evaluation through
    /// [`lookup`](Self::lookup), but they must not participate in pattern-variable equality checks.
    #[must_use]
    pub fn lookup_until_barrier(&self, sym: SymId) -> Option<TreeId> {
        self.lookup_until_barrier_value(sym)
            .and_then(|value| match value {
                EvalValue::Box(id) => Some(id),
                EvalValue::Closure(_) => None,
                EvalValue::PatternMatcher(_) => None,
            })
    }

    /// Value-returning core of [`lookup_until_barrier`](Self::lookup_until_barrier).
    ///
    /// Like [`lookup_value`](Self::lookup_value) but stops at the first barrier
    /// layer, returning the raw [`EvalValue`] instead of only boxes.
    pub(crate) fn lookup_until_barrier_value(&self, sym: SymId) -> Option<EvalValue> {
        self.with_store(|store| {
            let mut env_id = Some(self.current);
            while let Some(id) = env_id {
                let layer = &store.layers[id];
                if let Some(&index) = layer.binding_index.get(&sym) {
                    return Some(layer.bindings[index].1.clone());
                }
                if layer.barrier {
                    return None;
                }
                env_id = layer.parent;
            }
            None
        })
    }

    /// Looks up a symbol in the **current scope only**, without consulting any parent.
    ///
    /// This is used by `bind_definitions` to detect conflicting redefinitions of the same symbol
    /// within the same lexical layer — equivalent to the duplicate check inside C++ `addLayerDef`:
    ///
    /// ```cpp
    /// // environment.cpp — addLayerDef
    /// Tree olddef = nullptr;
    /// if (getProperty(lenv, id, olddef)) {   // ← checks only the current layer
    ///     if (def == olddef) { /* silent */ }
    ///     else { throw faustexception("redefinition …"); }
    /// }
    /// ```
    ///
    /// Unlike [`lookup`](Self::lookup), this method does **not** recurse to the parent, so
    /// re-binding a name that exists in an outer scope (shadowing) is correctly allowed.
    ///
    /// `sym` must be a valid id returned by [`TreeArena::intern_symbol`] or
    /// [`TreeArena::get_symbol`].
    #[must_use]
    pub fn lookup_local(&self, sym: SymId) -> Option<TreeId> {
        self.lookup_local_value(sym).and_then(|value| match value {
            EvalValue::Box(id) => Some(id),
            EvalValue::Closure(_) => None,
            EvalValue::PatternMatcher(_) => None,
        })
    }

    /// Value-returning core of [`lookup_local`](Self::lookup_local).
    ///
    /// Consults only the current layer (no parent traversal) and returns the raw
    /// [`EvalValue`] rather than only boxes.
    pub(crate) fn lookup_local_value(&self, sym: SymId) -> Option<EvalValue> {
        self.with_store(|store| {
            let layer = &store.layers[self.current];
            layer
                .binding_index
                .get(&sym)
                .map(|&index| layer.bindings[index].1.clone())
        })
    }

    /// Creates a new child scope whose parent is `self`.
    ///
    /// The child starts with an empty `bindings` vec. Any symbol bound in the child shadows the
    /// same-named symbol in the parent for the lifetime of the child scope.
    ///
    /// **C++ equivalent**: `pushNewLayer(lenv)` in `environment.cpp`:
    /// ```cpp
    /// static Tree pushNewLayer(Tree lenv) {
    ///     return tree(unique("ENV_LAYER"), lenv);  // new node, parent = lenv
    /// }
    /// ```
    ///
    /// **Cost**: `O(1)` layer allocation in the shared environment arena.
    #[must_use]
    pub fn push_scope(&self) -> Self {
        self.push_child(false)
    }

    /// Creates a new child scope that acts as a pattern-matching barrier.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/environment.cpp`
    /// - `pushEnvBarrier`
    ///
    /// Normal lookup through [`lookup`](Self::lookup) still crosses this node, so rule RHS
    /// evaluation can see the outer lexical environment. Only
    /// [`lookup_until_barrier`](Self::lookup_until_barrier) stops here, matching C++
    /// `searchIdDef`.
    #[must_use]
    pub fn push_barrier_scope(&self) -> Self {
        self.push_child(true)
    }

    /// Shared core of [`push_scope`](Self::push_scope) and
    /// [`push_barrier_scope`](Self::push_barrier_scope): spawns a child whose
    /// parent is the current layer, with the given barrier flag.
    fn push_child(&self, barrier: bool) -> Self {
        self.spawn_child_with_parent(Some(self.current), barrier)
    }

    /// Allocates a fresh layer in the shared store and returns a handle to it.
    ///
    /// The low-level primitive behind [`push_scope`](Self::push_scope) and
    /// [`push_barrier_scope`](Self::push_barrier_scope): `parent` sets the lookup
    /// chain (`None` for a root layer) and `barrier` marks it as a pattern-matching
    /// barrier. The new handle shares the same store and source context.
    pub(crate) fn spawn_child_with_parent(&self, parent: Option<EnvId>, barrier: bool) -> Self {
        let current = self.with_store_mut(|store| {
            let next_id = store.layers.len();
            store.layers.push(EnvLayer {
                bindings: Vec::new(),
                binding_index: ahash::HashMap::with_hasher(ahash::RandomState::new()),
                parent,
                barrier,
            });
            next_id
        });
        Self {
            store: Rc::clone(&self.store),
            current,
            source_context: Arc::clone(&self.source_context),
        }
    }

    /// Clones the current layer's contents for inspection or replay.
    ///
    /// Returns the layer's parent, its barrier flag, and a copy of its bindings.
    pub(crate) fn layer_snapshot(&self) -> (Option<EnvId>, bool, Vec<(SymId, EvalValue)>) {
        self.with_store(|store| {
            let layer = &store.layers[self.current];
            (layer.parent, layer.barrier, layer.bindings.clone())
        })
    }

    /// Returns names bound in the **current scope layer only** (no parent traversal).
    ///
    /// Used by [`EvalError::UndefinedSymbol`](crate::EvalError::UndefinedSymbol) to populate the `local_scope` diagnostic field.
    /// Names are sorted and deduplicated for stable diagnostic output.
    ///
    /// `arena` is required to resolve interned `u32` symbol ids back to their string names for
    /// human-readable diagnostics.
    #[must_use]
    pub fn local_names(&self, arena: &TreeArena) -> Vec<String> {
        let mut out = self.with_store(|store| {
            store.layers[self.current]
                .bindings
                .iter()
                .filter_map(|(sym, _)| arena.symbol_name(*sym).map(str::to_owned))
                .collect::<Vec<_>>()
        });
        out.sort();
        out.dedup();
        out
    }

    /// Returns all names **visible from this scope** across the full parent chain.
    ///
    /// Used by [`EvalError::UndefinedSymbol`](crate::EvalError::UndefinedSymbol) to populate the `visible_scope` diagnostic field.
    /// Names are sorted and deduplicated — a name shadowed in an inner scope appears only once.
    #[must_use]
    pub fn visible_names(&self, arena: &TreeArena) -> Vec<String> {
        let mut out = self.with_store(|store| {
            let mut names = Vec::new();
            let mut env_id = Some(self.current);
            while let Some(id) = env_id {
                let layer = &store.layers[id];
                names.extend(
                    layer
                        .bindings
                        .iter()
                        .filter_map(|(sym, _)| arena.symbol_name(*sym).map(str::to_owned)),
                );
                env_id = layer.parent;
            }
            names
        });
        out.sort();
        out.dedup();
        out
    }

    /// Returns names from the **root (top-level) scope** by walking up the parent chain.
    ///
    /// Used by [`EvalError::UndefinedSymbol`](crate::EvalError::UndefinedSymbol) to populate the `top_level_scope` diagnostic field,
    /// helping users see what top-level definitions are available when a symbol is not found.
    #[must_use]
    pub fn top_level_names(&self, arena: &TreeArena) -> Vec<String> {
        let mut out = self.with_store(|store| {
            let mut env_id = self.current;
            while let Some(parent) = store.layers[env_id].parent {
                env_id = parent;
            }
            store.layers[env_id]
                .bindings
                .iter()
                .filter_map(|(sym, _)| arena.symbol_name(*sym).map(str::to_owned))
                .collect::<Vec<_>>()
        });
        out.sort();
        out.dedup();
        out
    }

    /// Runs `f` with shared (read-only) access to the underlying [`EnvStore`].
    ///
    /// Centralizes the `RefCell` borrow so call sites never hold a borrow guard
    /// across other operations on the same environment.
    pub(crate) fn with_store<R>(&self, f: impl FnOnce(&EnvStore) -> R) -> R {
        let guard = self.store.borrow();
        f(&guard)
    }

    /// Runs `f` with exclusive (mutable) access to the underlying [`EnvStore`].
    ///
    /// The mutable counterpart of [`with_store`](Self::with_store).
    pub(crate) fn with_store_mut<R>(&self, f: impl FnOnce(&mut EnvStore) -> R) -> R {
        let mut guard = self.store.borrow_mut();
        f(&mut guard)
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::empty()
    }
}
