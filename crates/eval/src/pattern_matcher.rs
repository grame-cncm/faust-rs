//! Tree-automaton-based pattern matcher for Faust `case` rules.
//!
//! # C++ source correspondence
//!
//! | Rust symbol | C++ source |
//! |---|---|
//! | [`make_pattern_matcher`] | `make_pattern_matcher()` in `compiler/patternmatcher/patternmatcher.cpp` |
//! | [`apply_pattern_matcher`] | `apply_pattern_matcher()` in `compiler/patternmatcher/patternmatcher.cpp` |
//! | [`Automaton`] | `struct Automaton` / `Garbageable` subclass in same file |
//! | [`State`] | `struct State` |
//! | [`Trans`] / [`TransKind`] | `struct Trans` (`.x`, `.n`, `.arity`, `.state`) |
//! | [`Rule`] | `struct Rule` (`.r`, `.id`, `.p`) |
//!
//! # Algorithm overview
//!
//! Implements the **incremental Graef algorithm** (RTA 1991) to compile a set of Faust
//! `case` rules into a single deterministic **tree automaton**, then applies that automaton
//! to match a sequence of evaluated arguments against all rules simultaneously.
//!
//! ## Construction — [`make_pattern_matcher`]
//!
//! For each rule (in source order after reversing the stored reversed list):
//! 1. Build a **per-rule trie** by calling `make_state` for each pattern in the LHS.
//!    Each pattern extends the trie one step: variables (`PatternVar`) emit a `Var`
//!    transition; binary/ternary operators emit `Op` transitions; everything else emits
//!    a `Constant` transition.
//! 2. Merge that trie into the shared automaton via `merge_state` (deterministic union
//!    of states/transitions, preserving the variable-first invariant).
//! 3. After all rules, `build_automaton_metadata` propagates `match_num` flags (whether a
//!    state has a numeric-constant transition) for potential future optimisations.
//!
//! ## Matching — [`apply_pattern_matcher`]
//!
//! **Called once per consumed argument** in a loop from the evaluator's `apply_case_rules`.
//! Each call:
//! 1. `apply_pattern_matcher_internal` descends the argument tree, following state-machine
//!    transitions and recording variable-to-path associations (`Assoc`) in a per-rule
//!    `substs` table.
//! 2. After traversal, variable substitutions are resolved: for each active rule the path
//!    is used to extract the matched subterm, and the result is bound into the rule's
//!    `Environment` in `env_out`. **Nonlinearity** (same variable matched against two
//!    different values) nulls out `env_out[r]`.
//! 3. Returns `(new_state, Some(rhs))` when the automaton reaches a **final state** (all
//!    patterns consumed), or `(new_state, None)` for an intermediate state, or `(-1, None)`
//!    on failure.
//!
//! After the last argument is processed, the caller picks the **first rule** whose
//! `env_out[r]` is still `Some` and evaluates its RHS in that environment.
//!
//! ## Transition ordering invariant
//!
//! Within each state's `trans` list:
//! 1. At most one **`Var`** transition — always at index 0 if present (fallback for any value).
//! 2. **`Constant`** transitions sorted by ascending `TreeId::as_u32()`.
//! 3. **`Op`** transitions sorted by ascending `(arity, tag)`.
//!
//! This order mirrors the C++ implementation and ensures deterministic matching.

use ahash::AHashMap;
use boxes::{BoxMatch, match_box};
use tlib::{NodeKind, TreeArena, TreeId};

use crate::Environment;

// ── Public types ─────────────────────────────────────────────────────────────

/// Subterm path: a sequence of child indices traversed to locate a variable's value.
///
/// A path `[0, 1]` means "take the first child, then its second child."
/// Used in [`Rule`] to record where a pattern variable's bound value lives inside
/// the matched argument tree, so that the value can be extracted at match time via
/// [`subtree`].
pub type Path = Vec<usize>;

/// Active-rule marker stored inside an automaton state.
///
/// Each `Rule` entry inside a [`State`] says: "rule `r` is still compatible with the
/// transitions taken so far." If the rule contains a pattern variable, `id` holds the
/// variable's identifier node (inner `TreeId` of the `PatternVar`) and `p` records the
/// subterm path needed to extract the matched value at the end of traversal.
///
/// # C++ correspondence
/// `struct Rule { int r; Tree id; Path p; }` in `patternmatcher.cpp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// Rule index (0-based, source order after reversal).
    pub r: usize,
    /// Pattern variable identifier node, or `None` for non-variable positions.
    pub id: Option<TreeId>,
    /// Subterm path to the matched variable's value within the argument tree.
    pub p: Path,
}

impl Rule {
    /// Creates a new rule marker.
    pub fn new(r: usize, id: Option<TreeId>, p: Path) -> Self {
        Self { r, id, p }
    }
}

/// The kind of a transition in the automaton.
///
/// Transitions are stored inside [`Trans`] and determine which argument values
/// cause a state change. The **variable** transition (`Var`) is the catch-all:
/// it matches any value and is always tried last (it is stored first in the list
/// but skipped until all specific transitions fail).
///
/// # C++ correspondence
/// `Trans::is_var_trans()`, `Trans::is_cst_trans()`, `Trans::is_op_trans()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransKind {
    /// Matches any value. Stored at position 0 when present. C++: `x == nullptr && arity == 0`.
    Var,
    /// Matches a specific 0-arity constant (numeric literal or atom). C++: `x != nullptr && arity == 0`.
    Constant(TreeId),
    /// Matches an operator node with the given tag and arity. C++: `arity > 0`.
    Op {
        /// Tag id as interned by [`TreeArena::intern_tag`].
        tag: u32,
        /// Number of children (2 for binary operators, 3 for [`BoxMatch::Route`]).
        arity: usize,
    },
}

/// A single outgoing transition from an automaton state.
///
/// Transitions form the edges of the deterministic tree automaton. Each state holds
/// a `Vec<Trans>` ordered by the [invariant](crate::pattern_matcher#transition-ordering-invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trans {
    /// What kind of input this transition accepts.
    pub kind: TransKind,
    /// Index into [`Automaton::states`] of the successor state.
    pub state: usize,
}

impl Trans {
    /// Returns `true` if this is a variable (catch-all) transition.
    pub fn is_var(&self) -> bool {
        matches!(self.kind, TransKind::Var)
    }

    /// Returns the constant `TreeId` if this is a constant transition, `None` otherwise.
    pub fn is_const(&self) -> Option<TreeId> {
        match self.kind {
            TransKind::Constant(x) => Some(x),
            _ => None,
        }
    }

    /// Returns `(tag, arity)` if this is an operator transition, `None` otherwise.
    pub fn is_op(&self) -> Option<(u32, usize)> {
        match self.kind {
            TransKind::Op { tag, arity } => Some((tag, arity)),
            _ => None,
        }
    }
}

/// A state in the compiled tree automaton.
///
/// Each state records which rules are still compatible (`rules`) and which transitions
/// are available (`trans`). A state with no transitions is a **final state** — all
/// patterns of at least one rule have been fully consumed.
///
/// # C++ correspondence
/// `struct State { int s; bool match_num; list<Rule> rules; list<Trans> trans; }`.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// State index (equals position in [`Automaton::states`]).
    pub s: usize,
    /// `true` when at least one outgoing constant transition targets a numeric literal.
    /// Reserved for future numeric-fast-path optimisation; mirrors the C++ `match_num` flag.
    pub match_num: bool,
    /// Rules still active in this state (subset that have not yet failed to match).
    pub rules: Vec<Rule>,
    /// Outgoing transitions, ordered: `[Var?] [Constants…] [Ops…]`.
    pub trans: Vec<Trans>,
}

/// The compiled pattern-matching automaton for a `case` block.
///
/// Built once by [`make_pattern_matcher`] for a given set of rules and then
/// re-used for every application of that `case` node. Rules are stored in
/// source order (after reversing the cons-list); the `rhs` vector is indexed by
/// rule number `r`.
///
/// # C++ correspondence
/// `struct Automaton { vector<State*> state; vector<Tree> rhs; … }`.
#[derive(Clone, Debug, Default)]
pub struct Automaton {
    /// All states of the automaton, indexed by `usize`.
    pub states: Vec<State>,
    /// Right-hand side expression tree for each rule (indexed by rule number `r`).
    pub rhs: Vec<TreeId>,
}

impl Automaton {
    /// Creates an empty automaton.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of rules compiled into this automaton.
    pub fn n_rules(&self) -> usize {
        self.rhs.len()
    }

    /// Returns `true` when state `s` has no outgoing transitions (all patterns consumed).
    pub fn final_state(&self, s: usize) -> bool {
        self.states[s].trans.is_empty()
    }

    /// Allocates a fresh state and returns its index.
    pub fn new_state(&mut self) -> usize {
        let idx = self.states.len();
        self.states.push(State {
            s: idx,
            ..Default::default()
        });
        idx
    }
}

/// Cache keyed by the `TreeId` of an evaluated `Case` rule-list.
///
/// Raw parser rule-lists are not sufficient keys because pattern evaluation can
/// depend on the surrounding lexical environment. `apply_case_rules` therefore
/// evaluates/simplifies patterns first and only then memoises the compiled
/// automaton under the resulting hash-consed rule-list `TreeId`.
pub(crate) type AutomatonCache = AHashMap<TreeId, Automaton>;

// ── Internal pattern helpers ──────────────────────────────────────────────────

/// Deconstructs a cons-cell into `(head, tail)`, returning `None` for nil or non-list nodes.
fn is_cons(arena: &TreeArena, list: TreeId) -> Option<(TreeId, TreeId)> {
    // is_list returns true for both nil and cons; hd/tl return None for nil.
    if arena.is_list(list) {
        Some((arena.hd(list)?, arena.tl(list)?))
    } else {
        None
    }
}

/// Extracts the numeric tag of a `NodeKind::Tag` node, or `None` for other kinds.
fn get_node_tag(arena: &TreeArena, id: TreeId) -> Option<u32> {
    match arena.node(id)?.kind {
        NodeKind::Tag(tag) => Some(tag),
        _ => None,
    }
}

/// Returns `(tag, left, right)` if `box_id` is a binary-operator pattern node.
///
/// Recognised operators: `Seq`, `Par`, `Split`, `Merge`, `HGroup`, `VGroup`, `TGroup`, `Rec`.
fn is_box_pattern_op_binary(arena: &TreeArena, box_id: TreeId) -> Option<(u32, TreeId, TreeId)> {
    match match_box(arena, box_id) {
        BoxMatch::Seq(t1, t2)
        | BoxMatch::Par(t1, t2)
        | BoxMatch::Split(t1, t2)
        | BoxMatch::Merge(t1, t2)
        | BoxMatch::HGroup(t1, t2)
        | BoxMatch::VGroup(t1, t2)
        | BoxMatch::TGroup(t1, t2)
        | BoxMatch::Rec(t1, t2) => Some((get_node_tag(arena, box_id)?, t1, t2)),
        _ => None,
    }
}

/// Returns `(tag, a, b, c)` if `box_id` is a ternary-operator pattern node (`Route`).
fn is_box_pattern_op_ternary(
    arena: &TreeArena,
    box_id: TreeId,
) -> Option<(u32, TreeId, TreeId, TreeId)> {
    match match_box(arena, box_id) {
        BoxMatch::Route(t1, t2, t3) => Some((get_node_tag(arena, box_id)?, t1, t2, t3)),
        _ => None,
    }
}

/// Returns the inner identifier node if `box_id` is a `PatternVar(_)`.
fn is_box_pattern_var(arena: &TreeArena, box_id: TreeId) -> Option<TreeId> {
    match match_box(arena, box_id) {
        BoxMatch::PatternVar(id) => Some(id),
        _ => None,
    }
}

/// Returns `true` when `box_id` is a numeric literal (`Int` or `Real`).
fn is_box_num(arena: &TreeArena, box_id: TreeId) -> bool {
    matches!(
        match_box(arena, box_id),
        BoxMatch::Int(_) | BoxMatch::Real(_)
    )
}

// ── Subterm extraction ────────────────────────────────────────────────────────

/// Follows a subterm [`Path`] from position `i` inside argument tree `x`.
///
/// Descends into ternary or binary operator children according to the index stored
/// at each path position. Returns `x` unchanged when `i >= p.len()` (leaf reached).
///
/// # C++ correspondence
/// `static Tree subtree(Tree X, int i, const Path& p)` in `patternmatcher.cpp`.
fn subtree(arena: &TreeArena, x: TreeId, i: usize, p: &Path) -> TreeId {
    if i >= p.len() {
        return x;
    }
    if let Some((_, x0, x1, x2)) = is_box_pattern_op_ternary(arena, x) {
        return match p[i] {
            0 => subtree(arena, x0, i + 1, p),
            1 => subtree(arena, x1, i + 1, p),
            _ => subtree(arena, x2, i + 1, p),
        };
    }
    if let Some((_, x0, x1)) = is_box_pattern_op_binary(arena, x) {
        let child = if p[i] == 0 { x0 } else { x1 };
        return subtree(arena, child, i + 1, p);
    }
    x
}

// ── Automaton construction ────────────────────────────────────────────────────

/// Builds the trie fragment for a single pattern `x` starting from `state_idx`.
///
/// Mutates `p` (the current path) during recursion — callers must `push`/`pop`
/// around child calls. Returns the index of the final state after processing `x`.
///
/// # C++ correspondence
/// `static State* make_state(State*, int r, Tree x, Path& p)`.
fn make_state(
    arena: &TreeArena,
    automaton: &mut Automaton,
    state_idx: usize,
    r: usize,
    x: TreeId,
    p: &mut Path,
) -> usize {
    if let Some(id) = is_box_pattern_var(arena, x) {
        // Variable pattern `_name`: records (rule, id, path) and emits a Var transition.
        automaton.states[state_idx]
            .rules
            .push(Rule::new(r, Some(id), p.clone()));
        let next = automaton.new_state();
        automaton.states[state_idx].trans.push(Trans {
            kind: TransKind::Var,
            state: next,
        });
        next
    } else if let Some((op, x0, x1, x2)) = is_box_pattern_op_ternary(arena, x) {
        // Ternary operator (Route): emit Op(arity=3) then recurse into each child.
        automaton.states[state_idx]
            .rules
            .push(Rule::new(r, None, Vec::new()));
        let next = automaton.new_state();
        automaton.states[state_idx].trans.push(Trans {
            kind: TransKind::Op { tag: op, arity: 3 },
            state: next,
        });
        let mut cur = next;
        p.push(0);
        cur = make_state(arena, automaton, cur, r, x0, p);
        p.pop();
        p.push(1);
        cur = make_state(arena, automaton, cur, r, x1, p);
        p.pop();
        p.push(2);
        cur = make_state(arena, automaton, cur, r, x2, p);
        p.pop();
        cur
    } else if let Some((op, x0, x1)) = is_box_pattern_op_binary(arena, x) {
        // Binary operator (Seq, Par, …): emit Op(arity=2) then recurse into each child.
        automaton.states[state_idx]
            .rules
            .push(Rule::new(r, None, Vec::new()));
        let next = automaton.new_state();
        automaton.states[state_idx].trans.push(Trans {
            kind: TransKind::Op { tag: op, arity: 2 },
            state: next,
        });
        let mut cur = next;
        p.push(0);
        cur = make_state(arena, automaton, cur, r, x0, p);
        p.pop();
        p.push(1);
        cur = make_state(arena, automaton, cur, r, x1, p);
        p.pop();
        cur
    } else {
        // Constant (literal, atom): emit a Constant transition.
        automaton.states[state_idx]
            .rules
            .push(Rule::new(r, None, Vec::new()));
        let next = automaton.new_state();
        automaton.states[state_idx].trans.push(Trans {
            kind: TransKind::Constant(x),
            state: next,
        });
        next
    }
}

/// Clones `template_state_idx` and prefixes it with `n` chained `Var` transitions.
///
/// Used when merging a variable pattern into a state that already has specific transitions:
/// the variable must propagate into every existing successor state (for operators, one
/// `Var` transition per child).
///
/// Intuition: if a rule contains a variable where another rule contains an
/// operator of arity `n`, then the variable must remain compatible with the full
/// `n`-child descent taken by that operator rule. The cloned prefix therefore
/// acts as a synthetic "match anything for the next `n` structural steps"
/// scaffold before control rejoins the original template state.
///
/// # C++ correspondence
/// `static State* make_var_state(int n, State*)`.
fn make_var_state(automaton: &mut Automaton, n: usize, template_state_idx: usize) -> usize {
    if n == 0 {
        let new_idx = automaton.new_state();
        let template = automaton.states[template_state_idx].clone();
        automaton.states[new_idx] = template;
        automaton.states[new_idx].s = new_idx;
        return new_idx;
    }

    // Build anonymous rule markers (no id, no path) for the prefix chain.
    let prefix_rules: Vec<Rule> = automaton.states[template_state_idx]
        .rules
        .iter()
        .map(|r| Rule {
            r: r.r,
            id: None,
            p: Vec::new(),
        })
        .collect();

    let start_idx = automaton.new_state();
    let mut cur = start_idx;
    for _ in 0..n {
        automaton.states[cur].rules = prefix_rules.clone();
        let next = automaton.new_state();
        automaton.states[cur].trans.push(Trans {
            kind: TransKind::Var,
            state: next,
        });
        cur = next;
    }
    let template = automaton.states[template_state_idx].clone();
    automaton.states[cur] = template;
    automaton.states[cur].s = cur;
    start_idx
}

/// Merges the rule list `rules2` into `rules1`, keeping it sorted by rule index.
///
/// Both lists are sorted by [`Rule::r`] (source order); the merge is stable and,
/// for equal indices, keeps the elements of `rules1` before those of `rules2`.
/// A final state picks the **first** surviving rule of its list, so this order
/// is what makes the first matching rule in textual order win: appending
/// instead would let a later, more specific rule (`f(0)`) shadow an earlier
/// general one (`f(n)`) whenever the specific rule's trie is built first and
/// the general rule's completion is merged into it.
///
/// # C++ correspondence
/// `static void merge_rules(list<Rule>&, list<Rule>&)`, which calls
/// `std::list::merge` with `Rule::operator<` comparing the rule indices.
fn merge_rules(rules1: &mut Vec<Rule>, rules2: Vec<Rule>) {
    if rules2.is_empty() {
        return;
    }
    let left = std::mem::take(rules1);
    let mut merged = Vec::with_capacity(left.len() + rules2.len());
    let mut a = left.into_iter().peekable();
    let mut b = rules2.into_iter().peekable();
    loop {
        match (a.peek(), b.peek()) {
            (Some(x), Some(y)) => {
                // `std::list::merge` moves an element of the second list only
                // when it is strictly smaller.
                if y.r < x.r {
                    merged.push(b.next().expect("peeked"));
                } else {
                    merged.push(a.next().expect("peeked"));
                }
            }
            (Some(_), None) => merged.extend(a.by_ref()),
            (None, Some(_)) => merged.extend(b.by_ref()),
            (None, None) => break,
        }
    }
    *rules1 = merged;
}

/// Merges trie rooted at `state2_idx` into `state1_idx` (destructive union).
///
/// After merging, `state1_idx` contains the union of all rules and transitions from both
/// states. Delegates to `merge_trans_*` for the transition-kind-specific logic.
///
/// This is the determinization step of the construction algorithm: instead of
/// keeping one trie per rule, Rust mutates the shared automaton so identical
/// prefixes share states and divergent prefixes become ordered outgoing
/// transitions of one deterministic state.
///
/// # C++ correspondence
/// `static void merge_state(State* s1, State* s2)`.
fn merge_state(automaton: &mut Automaton, state1_idx: usize, state2_idx: usize) {
    let rules2 = automaton.states[state2_idx].rules.clone();
    merge_rules(&mut automaton.states[state1_idx].rules, rules2);

    let trans2 = automaton.states[state2_idx].trans.clone();
    if trans2.is_empty() {
        return;
    }

    if automaton.states[state1_idx].trans.is_empty() {
        automaton.states[state1_idx].trans = trans2;
    } else if trans2[0].is_var() {
        merge_trans_var(automaton, state1_idx, trans2[0].state);
    } else if let Some(x) = trans2[0].is_const() {
        merge_trans_cst(automaton, state1_idx, x, trans2[0].state);
    } else if let Some((op, arity)) = trans2[0].is_op() {
        merge_trans_op(automaton, state1_idx, op, arity, trans2[0].state);
    }
}

/// Merges a `Var` transition (targeting `target_state_idx`) into `state1_idx`.
///
/// A variable pattern subsumes all existing transitions: the target state must
/// be merged into every successor of `state1_idx` so that rules using this variable
/// remain active regardless of the concrete value taken by the argument.
///
/// If `state1_idx` already has operator transitions, the variable path cannot be
/// merged directly into the operator successor because the operator rule will
/// still descend through child positions. [`make_var_state`] is therefore used
/// to manufacture a compatible number of `Var` hops before merging.
fn merge_trans_var(automaton: &mut Automaton, state1_idx: usize, target_state_idx: usize) {
    // Ensure a Var transition exists at position 0.
    if !automaton.states[state1_idx].trans[0].is_var() {
        let new_state = automaton.new_state();
        automaton.states[state1_idx].trans.insert(
            0,
            Trans {
                kind: TransKind::Var,
                state: new_state,
            },
        );
    }

    let trans_count = automaton.states[state1_idx].trans.len();
    for i in 0..trans_count {
        let (trans_state, trans_kind) = {
            let t = &automaton.states[state1_idx].trans[i];
            (t.state, t.kind.clone())
        };
        match trans_kind {
            TransKind::Var | TransKind::Constant(_) => {
                merge_state(automaton, trans_state, target_state_idx);
            }
            TransKind::Op { arity, .. } => {
                // Variable matches any Op: expand into `arity` Var children.
                let state_var = make_var_state(automaton, arity, target_state_idx);
                merge_state(automaton, trans_state, state_var);
            }
        }
    }
}

/// Merges a `Constant(x)` transition into `state1_idx`, inserting in sorted order.
///
/// If a transition for `x` already exists, recursively merges the successor states.
/// Otherwise creates a new successor (cloned from `target_state_idx`) and inserts it
/// at the correct sorted position, then propagates any existing Var transition into
/// the new successor.
///
/// The post-condition is that constant transitions remain strictly sorted by
/// `TreeId`, and any catch-all variable rule that was already active in
/// `state1_idx` also remains active after taking the new constant branch.
fn merge_trans_cst(
    automaton: &mut Automaton,
    state1_idx: usize,
    x: TreeId,
    target_state_idx: usize,
) {
    let mut insert_pos = 0;
    let mut found = false;
    let mut matching_trans_state = 0;

    {
        let trans = &automaton.states[state1_idx].trans;
        if !trans.is_empty() && trans[0].is_var() {
            insert_pos = 1;
        }
        let mut i = insert_pos;
        while i < trans.len() {
            if let Some(x1) = trans[i].is_const() {
                if x == x1 {
                    matching_trans_state = trans[i].state;
                    found = true;
                    break;
                } else if x.as_u32() < x1.as_u32() {
                    insert_pos = i;
                    break;
                }
            } else if trans[i].is_op().is_some() {
                insert_pos = i;
                break;
            }
            i += 1;
            insert_pos = i;
        }
    }

    if found {
        merge_state(automaton, matching_trans_state, target_state_idx);
    } else {
        let new_state = automaton.new_state();
        let template = automaton.states[target_state_idx].clone();
        automaton.states[new_state] = template;
        automaton.states[new_state].s = new_state;
        automaton.states[state1_idx].trans.insert(
            insert_pos,
            Trans {
                kind: TransKind::Constant(x),
                state: new_state,
            },
        );
        // Propagate existing Var transition into the new constant successor.
        if !automaton.states[state1_idx].trans.is_empty()
            && automaton.states[state1_idx].trans[0].is_var()
        {
            let var_state = automaton.states[state1_idx].trans[0].state;
            merge_state(automaton, new_state, var_state);
        }
    }
}

/// Merges an `Op { tag: op, arity }` transition into `state1_idx`, inserting sorted by `(arity, tag)`.
///
/// As with constants, an existing variable transition at `state1_idx` must keep
/// matching this new operator branch as well. The merge therefore expands the
/// variable continuation through `arity` synthetic `Var` edges before joining
/// the new operator successor.
fn merge_trans_op(
    automaton: &mut Automaton,
    state1_idx: usize,
    op: u32,
    arity: usize,
    target_state_idx: usize,
) {
    let mut insert_pos = 0;
    let mut found = false;
    let mut matching_trans_state = 0;

    {
        let trans = &automaton.states[state1_idx].trans;
        if !trans.is_empty() && trans[0].is_var() {
            insert_pos = 1;
        }
        while insert_pos < trans.len() && trans[insert_pos].is_const().is_some() {
            insert_pos += 1;
        }
        let mut i = insert_pos;
        while i < trans.len() {
            if let Some((op1, arity1)) = trans[i].is_op() {
                if arity < arity1 {
                    insert_pos = i;
                    break;
                } else if arity > arity1 {
                    // continue
                } else if op == op1 {
                    matching_trans_state = trans[i].state;
                    found = true;
                    break;
                } else if op < op1 {
                    insert_pos = i;
                    break;
                }
            }
            i += 1;
            insert_pos = i;
        }
    }

    if found {
        merge_state(automaton, matching_trans_state, target_state_idx);
    } else {
        let new_state = automaton.new_state();
        let template = automaton.states[target_state_idx].clone();
        automaton.states[new_state] = template;
        automaton.states[new_state].s = new_state;
        automaton.states[state1_idx].trans.insert(
            insert_pos,
            Trans {
                kind: TransKind::Op { tag: op, arity },
                state: new_state,
            },
        );
        // Propagate existing Var transition: expand into `arity` Var children.
        if !automaton.states[state1_idx].trans.is_empty()
            && automaton.states[state1_idx].trans[0].is_var()
        {
            let var_state = automaton.states[state1_idx].trans[0].state;
            let state2 = make_var_state(automaton, arity, var_state);
            merge_state(automaton, new_state, state2);
        }
    }
}

/// Propagates `match_num` flags downward from `state_idx` (DFS post-order).
///
/// A state's `match_num` is `true` if any of its outgoing constant transitions
/// targets a numeric literal. This mirrors the C++ `build()` / `match_num` logic
/// and is reserved for a future numeric-fast-path optimisation. It is metadata
/// only in the current Rust port and does not alter matching behavior yet.
fn build_automaton_metadata(arena: &TreeArena, automaton: &mut Automaton, state_idx: usize) {
    // Collect indices first to avoid borrow aliasing.
    let trans_indices: Vec<usize> = (0..automaton.states[state_idx].trans.len()).collect();
    let mut match_num = false;
    for i in trans_indices {
        let next_state = automaton.states[state_idx].trans[i].state;
        if let Some(x) = automaton.states[state_idx].trans[i].is_const()
            && is_box_num(arena, x)
        {
            match_num = true;
        }
        build_automaton_metadata(arena, automaton, next_state);
    }
    automaton.states[state_idx].match_num = match_num;
}

// ── Public construction ───────────────────────────────────────────────────────

/// Compiles a `case` rule list into a deterministic tree automaton.
///
/// # Arguments
///
/// * `arena` — the hash-consing arena (needed for pattern deconstruction).
/// * `rules` — a `TreeId` pointing to the **cons-list** of rules as stored in the `Case` node.
///   Each element is a `cons(lhs, rhs)` pair where `lhs` is itself a cons-list of patterns.
///   The list is in **reversed source order** (as the parser stores it); this function
///   reverses it before processing so that rule 0 has the highest textual priority.
///
/// # Returns
///
/// An [`Automaton`] ready to be fed arguments one at a time via [`apply_pattern_matcher`].
///
/// # C++ correspondence
///
/// `Automaton* make_pattern_matcher(Tree R)` in `patternmatcher.cpp`.
pub fn make_pattern_matcher(arena: &mut TreeArena, rules: TreeId) -> Automaton {
    let mut automaton = Automaton::new();
    let start_state = automaton.new_state();

    // Collect the cons-list into a Vec, then reverse to restore source order.
    let mut rules_vec = Vec::new();
    let mut curr = rules;
    while let Some((rule, rest)) = is_cons(arena, curr) {
        rules_vec.push(rule);
        curr = rest;
    }
    rules_vec.reverse();

    for (r, rule) in rules_vec.iter().enumerate() {
        let Some((lhs, rhs)) = is_cons(arena, *rule) else {
            continue;
        };
        automaton.rhs.push(rhs);

        // Collect and reverse the LHS pattern list.
        let mut pats = Vec::new();
        let mut curr_lhs = lhs;
        while let Some((pat, rest)) = is_cons(arena, curr_lhs) {
            pats.push(pat);
            curr_lhs = rest;
        }
        pats.reverse();

        // Build per-rule trie and merge into the shared automaton.
        let state0 = automaton.new_state();
        let mut state = state0;
        for pat in pats {
            let mut p = Vec::new();
            state = make_state(arena, &mut automaton, state, r, pat, &mut p);
        }
        automaton.states[state]
            .rules
            .push(Rule::new(r, None, Vec::new()));
        merge_state(&mut automaton, start_state, state0);
    }

    build_automaton_metadata(arena, &mut automaton, start_state);
    automaton
}

// ── Matching internals ────────────────────────────────────────────────────────

/// Variable-to-path association recorded during automaton traversal.
#[derive(Clone)]
struct Assoc {
    /// Inner identifier `TreeId` of the `PatternVar` node.
    id: TreeId,
    /// Subterm path from the top-level argument to the matched value.
    p: Path,
}

/// Per-rule substitution table: for each rule index, the list of `(id, path)` pairs
/// recorded when the automaton passed through variable transitions for that rule.
type Subst = Vec<Assoc>;

/// Records variable-binding associations for all rules active in state `s`.
///
/// Called at each step of `apply_pattern_matcher_internal` when a transition succeeds,
/// so that all still-active rules accumulate their variable paths.
/// The actual matched subtree is not extracted here; only `(identifier, path)`
/// metadata is recorded. Extraction happens later in [`apply_pattern_matcher`]
/// once the whole top-level argument has been accepted.
fn add_subst(automaton: &Automaton, s: usize, substs: &mut [Subst]) {
    for r in &automaton.states[s].rules {
        if let Some(id) = r.id {
            substs[r.r].push(Assoc { id, p: r.p.clone() });
        }
    }
}

/// Recursively matches argument `x` against automaton state `s`, returning the successor state.
///
/// Non-variable transitions are tried first (constant, then operator). If none match, the
/// variable transition at position 0 is used as a catch-all. Returns `None` on failure.
///
/// On every successful step, [`add_subst`] records the variable-path
/// associations attached to the *current* state before descending further. This
/// mirrors the C++ matcher, where substitution information is accumulated during
/// traversal and only materialized into environment bindings once the top-level
/// argument has been fully processed.
///
/// # C++ correspondence
/// `static int apply_pattern_matcher_internal(Automaton*, int s, Tree X, vector<Subst>&)`.
fn apply_pattern_matcher_internal(
    arena: &mut TreeArena,
    automaton: &Automaton,
    s: usize,
    x: TreeId,
    substs: &mut [Subst],
) -> Option<usize> {
    let state = &automaton.states[s];

    // Top-level simplification is in `apply_pattern_matcher` (so variable
    // bindings also receive the simplified value). This second check handles
    // recursive sub-tree matching inside Op patterns — e.g., matching the `2`
    // inside `seq(max(1,min(2,4)), x)` against a constant transition.
    let x = if state.match_num {
        crate::simplify_pattern(arena, x)
    } else {
        x
    };

    for trans in &state.trans {
        if trans.is_var() {
            continue; // tried last as fallback
        }
        if let Some(cst) = trans.is_const() {
            if x == cst {
                add_subst(automaton, s, substs);
                return Some(trans.state);
            }
        } else if let Some((op, _)) = trans.is_op() {
            // Ternary match (Route)
            if let Some((op1, x0, x1, x2)) = is_box_pattern_op_ternary(arena, x) {
                if op == op1 {
                    add_subst(automaton, s, substs);
                    let cur =
                        apply_pattern_matcher_internal(arena, automaton, trans.state, x0, substs)?;
                    let cur = apply_pattern_matcher_internal(arena, automaton, cur, x1, substs)?;
                    return apply_pattern_matcher_internal(arena, automaton, cur, x2, substs);
                }
            // Binary match (Seq, Par, …)
            } else if let Some((op1, x0, x1)) = is_box_pattern_op_binary(arena, x)
                && op == op1
            {
                add_subst(automaton, s, substs);
                let cur =
                    apply_pattern_matcher_internal(arena, automaton, trans.state, x0, substs)?;
                return apply_pattern_matcher_internal(arena, automaton, cur, x1, substs);
            }
        }
    }

    // Fallback: variable transition (always matches).
    if !state.trans.is_empty() && state.trans[0].is_var() {
        add_subst(automaton, s, substs);
        return Some(state.trans[0].state);
    }

    None
}

// ── Public matching API ───────────────────────────────────────────────────────

/// Applies the automaton to a single argument, advancing the state machine.
///
/// This function is called **once per consumed argument** inside a loop in
/// `apply_case_rules`. The `env_out` vector accumulates per-rule variable bindings
/// across calls (each call modifies `env_out` in-place; `env_out[r]` is set to `None`
/// when rule `r` fails a nonlinearity check).
///
/// Operationally, one call does three things:
/// 1. traverse the argument tree with [`apply_pattern_matcher_internal`],
/// 2. replay recorded paths to extract concrete matched subtrees,
/// 3. bind those subtrees into each surviving rule environment, rejecting
///    nonlinear matches where the same variable name would receive two distinct
///    values.
///
/// # Arguments
///
/// * `arena` — mutable arena, needed to intern pattern-variable names.
/// * `automaton` — the compiled automaton (from [`make_pattern_matcher`]).
/// * `s` — current automaton state (starts at `0` for the first argument).
/// * `x` — the evaluated argument tree to match.
/// * `env_out` — per-rule environment slots (slice, not `Vec`, for API clarity);
///   `env_out[r]` is `Some` while rule `r` is still a candidate, `None` if disqualified.
///
/// # Returns
///
/// `(new_state, maybe_rhs)`:
/// - `new_state`: `Some(successor_state_index)`, or `None` on complete match failure.
/// - `maybe_rhs`: `Some(rhs_tree)` when a **final state** is reached (all patterns
///   for at least one rule have been consumed), `None` otherwise.
///
/// # C++ correspondence
///
/// `int apply_pattern_matcher(Automaton*, int s, Tree X, Tree& C, vector<Tree>& E)`.
pub fn apply_pattern_matcher(
    arena: &mut TreeArena,
    automaton: &Automaton,
    s: usize,
    x: TreeId,
    env_out: &mut [Option<Environment>],
) -> (Option<usize>, Option<TreeId>) {
    let n = automaton.n_rules();
    let mut substs: Vec<Subst> = vec![Vec::new(); n];

    // C++ parity: simplify the argument to a numeric literal when the current
    // state has numeric constant transitions. This must happen HERE (not just
    // inside `apply_pattern_matcher_internal`) so that variable bindings also
    // receive the simplified value. Without this, a variable `i` matched against
    // `sub(max(1,min(2,4)),1)` would bind the unsimplified expression, causing
    // infinite recursion in recursive case rules like `factorial(i-1)`.
    let x = if automaton.states[s].match_num {
        crate::simplify_pattern(arena, x)
    } else {
        x
    };

    let s_idx = match apply_pattern_matcher_internal(arena, automaton, s, x, &mut substs) {
        None => return (None, None),
        Some(s) => s,
    };

    // Apply variable substitutions to per-rule environments.
    // Nonlinearity: if the same variable is already bound to a *different* value, disqualify
    // the rule by setting env_out[r] = None.
    // Collect rule indices to avoid borrow aliasing between `automaton` and `arena`.
    let rule_indices: Vec<usize> = automaton.states[s_idx].rules.iter().map(|r| r.r).collect();

    for &rule_r in &rule_indices {
        if env_out[rule_r].is_none() {
            continue;
        }
        for assoc in &substs[rule_r] {
            // Resolve the pattern variable's identifier node to a name string,
            // then intern it — collecting into String first to release the
            // `arena` immutable borrow before calling intern_symbol(&mut self).
            let name: Option<String> = match match_box(arena, assoc.id) {
                BoxMatch::Ident(n) => Some(n.to_string()),
                BoxMatch::PatternVar(inner) => match match_box(arena, inner) {
                    BoxMatch::Ident(n) => Some(n.to_string()),
                    _ => None,
                },
                _ => None,
            };
            let Some(name) = name else { continue };

            let z1 = subtree(arena, x, 0, &assoc.p);
            let sym_id = arena.intern_symbol(&name);

            if let Some(env) = env_out[rule_r].as_mut() {
                if let Some(existing) = env.lookup_until_barrier(sym_id) {
                    if existing != z1 {
                        // Nonlinearity failure: same variable matched two different values.
                        env_out[rule_r] = None;
                        break;
                    }
                } else {
                    env.bind(sym_id, z1);
                }
            }
        }
    }

    // If this is a final state, return the first surviving rule's RHS.
    if automaton.final_state(s_idx) {
        for r in &automaton.states[s_idx].rules {
            if env_out[r.r].is_some() {
                return (Some(s_idx), Some(automaton.rhs[r.r]));
            }
        }
        return (None, None);
    }

    (Some(s_idx), None)
}
