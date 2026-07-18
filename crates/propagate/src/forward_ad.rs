//! Forward-mode automatic differentiation helpers for the `propagate` phase.
//!
//! # Source provenance (C++)
//! - `compiler/propagate/propagate.cpp`
//! - `compiler/transform/sigtyperules.hh`
//! - `compiler/signals/signals.cpp`
//!
//! # Scope
//! - Differentiate a list of primal outputs with respect to a list of seed
//!   signals supplied by the caller.
//! - Expand each primal output into `primal + one tangent per seed`.
//! - Preserve shared signal DAG structure through memoized transformation.
//! - Keep reverse-mode (`rad`) explicitly out of scope for this phase.
//!
//! # Current architecture
//! [`ForwardADTransform`] carries one tangent lane per selected seed inside
//! one internal dual-number carrier, and [`generate_fad_signals_multi`]
//! drives one transformer for the whole seed set. This keeps the
//! differentiation rules, recursion bookkeeping, and seed lifting expressed
//! once in the transformer and lets recursive groups share one interleaved
//! `DEBRUIJNREC` instead of rebuilding one private primal shadow per seed.
//!
//! Internally, one differentiated signal is represented as:
//!
//! ```text
//! Dual {
//!   primal,
//!   tangents = [d/ds0, d/ds1, …, d/ds{N-1}]
//! }
//! ```
//!
//! where the seed order is exactly the order of the lowered seed outputs passed
//! by `fad(expr, seed_box)`.
//!
//! # Dual-number algebra
//! Forward-mode AD (FAD) propagates derivatives alongside values by carrying a
//! *tangent* component next to each signal. A **dual signal** is written
//! `u + ε·u'` where `ε² = 0`. A **seed** is any signal `s` chosen by the
//! caller; `ds/ds = 1` and every other independent input has seed `0`.
//!
//! A seed is not restricted to a UI control: it can be any `SigId` in the
//! signal DAG (a slider, a lambda-bound recursive input, an expression).
//! Seed recognition is pure `SigId` equality — the arena hash-conses every
//! node, so every external reference to the seed inside the primal body
//! shares the same `TreeId` as the seed argument and the equality check
//! fires at each occurrence. The transform short-circuits at that leaf and
//! never descends into the seed's own recursive body.
//!
//! Repeated seeds are preserved by lane index, not deduplicated semantically:
//! if the seed box lowers to `[s, s]`, the differentiated output bundle still
//! exposes two tangent lanes in that same order.
//!
//! When the seed expression carries free `DEBRUIJNREF` nodes, crossing a
//! `DEBRUIJNREC` scope shifts those references by one level. The transform
//! therefore lifts every active seed on entry into each REC body (and
//! restores the previous seed vector on exit) so `SigId` equality keeps
//! matching under the new scope.
//!
//! The transform maintains two internal seed views:
//!
//! - `diff_seeds: Vec<SigId>` preserves deterministic lane order,
//! - `diff_seed_index: AHashMap<SigId, SmallVec<[usize; 2]>>` provides the
//!   reverse lookup from one `SigId` to one or more tangent lanes.
//!
//! This keeps seed recognition explicit while avoiding a linear scan over the
//! seed list at every visited node.
//!
//! # Differentiation rules by node kind
//!
//! ## Constants and audio inputs
//!
//! | Node | Tangent |
//! |------|---------|
//! | `int(c)` | `0` |
//! | `real(c)` | `0.0` |
//! | `sigInput(_)` | `0.0` |
//!
//! Audio inputs are treated as independent of all UI controls.
//!
//! ## Seeds and UI controls
//!
//! Seed equality is checked *before* node-kind dispatch, so any node whose
//! `SigId` equals a selected seed returns `1.0` on the matching tangent lane
//! and `0.0` on every other lane.
//!
//! | Node | Tangent |
//! |------|---------|
//! | any signal `s` such that `s == seed_j` | lane `j` = `1.0`, every other lane = `0.0` |
//! | `hslider` / `vslider` / `numentry` (not a seed) | `0.0` |
//! | `button`, `checkbox` | `0.0` (discrete, not differentiable) |
//!
//! Under the explicit-seed model, `[autodiff:false]` metadata is ignored:
//! whether a signal is differentiated is decided by the caller's seed list,
//! not by per-control annotations. The metadata still parses without error
//! so legacy sources remain accepted.
//!
//! The zero-tangent treatment for controls is intentional and not just a
//! parser limitation:
//!
//! - `hslider` / `vslider` / `numentry` are ordinary continuous signals when
//!   they are selected as explicit seeds; otherwise they are treated as
//!   independent of the active differentiation variables;
//! - `button` / `checkbox` are discrete state/control sources, so the current
//!   FAD pass models them as piecewise-constant and returns zero tangents;
//! - `vbargraph` / `hbargraph` are metering sinks, not differentiated signal
//!   paths.
//!
//! ## Arithmetic binary operators (`BinOp`)
//!
//! | Operator | Tangent rule |
//! |----------|-------------|
//! | `x + y` | `x' + y'` |
//! | `x - y` | `x' - y'` |
//! | `x * y` | `x'·y + x·y'` (product rule) |
//! | `x / y` | `(x'·y − x·y') / y²` (quotient rule) |
//! | `x % y` | `x' − y'·⌊x/y⌋` |
//! | shifts, bitwise ops, comparisons | `0` (non-differentiable integer ops) |
//!
//! ## Unary transcendentals (chain rule: `d f(x)/dp = f'(x) · x'`)
//!
//! | Function | Derivative `f'(x)` | Tangent emitted |
//! |----------|--------------------|-----------------|
//! | `sin(x)` | `cos(x)` | `cos(x) * x'` |
//! | `cos(x)` | `−sin(x)` | `(0 - sin(x)) * x'` |
//! | `tan(x)` | `1 / cos²(x)` | `(1 / cos(x)²) * x'` |
//! | `exp(x)` | `exp(x)` | `exp(x) * x'` |
//! | `log(x)` | `1 / x` | `(1 / x) * x'` |
//! | `log10(x)` | `1 / (x · ln 10)` | `(1 / (x * log(10))) * x'` |
//! | `sqrt(x)` | `1 / (2 · √x)` | `(1 / (2 * sqrt(x))) * x'` |
//! | `abs(x)` | `x / |x|` (sign) | `(x / abs(x)) * x'` |
//! | `acos(x)` | `−1 / √(1 − x²)` | `(-1 / sqrt(1 - x²)) * x'` |
//! | `asin(x)` | `1 / √(1 − x²)` | `(1 / sqrt(1 - x²)) * x'` |
//! | `atan(x)` | `1 / (1 + x²)` | `(1 / (1 + x²)) * x'` |
//!
//! Note: `abs` is not differentiable at `x = 0`; the expression `x/|x|`
//! has undefined behaviour there and produces `NaN` or `±inf` at runtime.
//!
//! ## Binary math (`pow`, `min`, `max`)
//!
//! ### `pow(x, y)`
//! ```text
//! d/dp x^y = x^y · (y' · ln(x) + y · x' / x)
//! ```
//! Both `x` and `y` may depend on the active seeds. The primal term `x^y` and
//! the common scalar factors are hoisted once, then one tangent lane is emitted
//! per seed.
//!
//! ### `min(x, y)` / `max(x, y)`
//! ```text
//! d/dp min(x,y) = select2(x < y, x', y')
//! d/dp max(x,y) = select2(x > y, x', y')
//! ```
//! The selector comes from the primal comparison. The tangent is piecewise
//! constant and uses the chosen branch's tangent lane.
//!
//! ### `atan2(y, x)`
//! ```text
//! d/dp atan2(y, x) = (x·y' − y·x') / (x² + y²)
//! ```
//!
//! ### `fmod(x, y)`
//! ```text
//! d/dp fmod(x, y) = x' − y'·floor(x/y)
//! ```
//!
//! ### `remainder(x, y)`
//! ```text
//! d/dp remainder(x, y) = x' − y'·round(x/y)
//! ```
//!
//! ## Foreign functions (`FFun`)
//!
//! Foreign functions are matched by name against a known-differentiable set.
//! The FFUN descriptor carries one name per precision variant; all slots are
//! checked so the match is precision-agnostic (`tanhf` / `tanh` / `tanhl`
//! all identify the same mathematical operation).
//!
//! Recognized unary rules:
//!
//! | Function | Names (`f32` \| `f64` \| `ldbl`) | Tangent rule | Notes |
//! |----------|----------------------------------|--------------|-------|
//! | `tanh(x)` | `tanhf` / `tanh` / `tanhl` | `x' · (1 − tanh²(x))` | computed from primal |
//! | `sinh(x)` | `sinhf` / `sinh` / `sinhl` | `x' · cosh(x)` | `cosh(x)` rebuilt as `sqrt(1 + sinh²(x))` |
//! | `cosh(x)` | `coshf` / `cosh` / `coshl` | `x' · sinh(x)` | `sinh(x)` rebuilt from exponentials |
//! | `atanh(x)` | `atanhf` / `atanh` / `atanhl` | `x' / (1 − x²)` | |
//! | `asinh(x)` | `asinhf` / `asinh` / `asinhl` | `x' / sqrt(1 + x²)` | |
//! | `acosh(x)` | `acoshf` / `acosh` / `acoshl` | `x' / sqrt(x² − 1)` | |
//!
//! Unrecognized FFUN calls, non-unary FFUNs, table writes, soundfile
//! accessors, standalone waveforms, on-demand up/downsampling, `Gen`,
//! `PermVar`, `TempVar`, and every other unmatched signal variant fall
//! through to a zero tangent with the primal unchanged.
//!
//! ## Cast operators
//!
//! | Node | Tangent |
//! |------|---------|
//! | `float_cast(x)` | `float_cast(x')` |
//! | `int_cast(x)` | `0` (piecewise-constant step function) |
//! | `bit_cast(x)` | `0` (reinterpret-cast, semantically opaque) |
//!
//! These choices are deliberate approximation boundaries:
//!
//! - `float_cast` preserves the underlying numeric value and therefore forwards
//!   the tangent through the cast;
//! - `int_cast` introduces discontinuous truncation/rounding semantics, so the
//!   derivative is treated as zero rather than trying to model impulses at
//!   integer boundaries;
//! - `bit_cast` changes representation rather than mathematical value and is
//!   therefore outside the differentiable subset.
//!
//! ## Delay operators
//!
//! ### Unit delay `delay1(x)`
//! ```text
//! d/dp delay1(x) = delay1(x')
//! ```
//!
//! ### Variable delay `delay(x, d)`
//! ```text
//! d/dp x[n − d(n)] = x'[n − d(n)]
//!                  − d'(n) · ∇x[n − d(n)]
//! ```
//! where `∇x[n−d]` is approximated as `delay(x − delay1(x), d)`.
//!
//! ## Read-only table reads
//!
//! `SIGRDTBL(table, idx)` is differentiable when `table` is a read-only table
//! source:
//!
//! - `SIGWAVEFORM(...)`
//! - `SIGWRTBL(size, generator, nil, nil)` produced by read-only table forms
//!
//! The table payload is treated as constant data. FAD differentiates only
//! through the read address:
//!
//! ```text
//! y = rdtbl(T, i)
//! y' = dT/di(i) · i'
//! dT/di(i) ≈ (rdtbl(T, i + 1) - rdtbl(T, i - 1)) / 2
//! ```
//!
//! This is a deliberate Rust-side approximation model, not a claim of exact
//! source-level parity with the Faust C++ compiler.
//!
//! Important boundaries:
//!
//! - the read uses the existing runtime table semantics for bounds handling;
//!   FAD does not invent extra wrap/clamp rules around `i ± 1`
//! - table contents are not differentiated
//! - writable tables stay outside the differentiable subset and emit zero
//!   tangents even when wrapped in `SIGRDTBL`
//!
//! ## Control-flow nodes
//!
//! | Node | Tangent |
//! |------|---------|
//! | `select2(cond, x, y)` | `select2(cond, x', y')` |
//! | `prefix(init, x)` | `prefix(init', x')` |
//!
//! ## Projection and recursive groups (de Bruijn form)
//!
//! `propagate` always emits recursive groups in de Bruijn form
//! (`DEBRUIJNREC` / `DEBRUIJNREF` tag nodes). The transform never sees
//! symbolic form (`SYMREC` / `SYMREF`); `de_bruijn_to_sym` runs once in
//! `signal_prepare` *after* FAD, over all process outputs together.
//!
//! ### Rewrite rules (N seed lanes)
//!
//! | Expression | Rewrite |
//! |------------|---------|
//! | `rec([e₀, …, eₖ₋₁])` | `rec([e₀, e₀'_{s₀}, …, e₀'_{sₙ₋₁}, e₁, e₁'_{s₀}, …, eₖ₋₁, eₖ₋₁'_{sₙ₋₁}])` — body interleaved as `[primal, tangent₀, …, tangentₙ₋₁]` per slot |
//! | `proj(i, rec_rebuilt)` | primal: `proj(i·(1+N), …)` ; tangent lane j: `proj(i·(1+N)+1+j, …)` |
//! | `proj(i, ref(k ≤ depth))` | same slot remapping as above (ref points into an already-rebuilt group on this path) |
//! | `proj(i, ref(k > depth))` | primal: `proj(i, …)` (pass-through) ; every tangent lane: `0` (outer group not rewritten on this path) |
//!
//! FAD's index bookkeeping rests on three distinct notions:
//!
//! - **Level** (payload on `DEBRUIJNREF`): a static integer `k` meaning
//!   "the `k`-th enclosing `DEBRUIJNREC`, innermost = 1".
//! - **`debruijn_depth`** (traversal counter on [`ForwardADTransform`]):
//!   incremented only when FAD enters a `DEBRUIJNREC` body that it rewrites.
//! - **Slot index** (integer on a `Proj` node): selects one element from
//!   a REC body.
//!
//! For a transformer carrying `N` seed lanes, every differentiated recursion
//! body is interleaved as:
//!
//! ```text
//! [p0, t0_s0, t0_s1, …, p1, t1_s0, t1_s1, …]
//! ```
//!
//! so the original slot `i` maps to:
//!
//! - primal slot `i * (1 + N)`
//! - tangent slot `i * (1 + N) + 1 + seed_index`
//!
//! The projection arm classifies the group into three semantic cases:
//!
//! | Group | Meaning | Primal projection | Tangent projection |
//! |-------|---------|-------------------|--------------------|
//! | `DEBRUIJNREC` directly | the current transform just rebuilt this recursion | `proj(i * (1 + N), …)` | `proj(i * (1 + N) + 1 + lane, …)` |
//! | `DEBRUIJNREF(level <= debruijn_depth)` | points into a recursion already rewritten on this path | same as above | same as above |
//! | `DEBRUIJNREF(level > debruijn_depth)` | points to an enclosing recursion that this transform never entered | `proj(i, …)` | `0` on every lane |
//! | Other | defensive fallback outside the expected de Bruijn-only flow | `proj(i, …)` | `proj(i, …)` lane-wise |
//!
//! The `level > debruijn_depth` case is the subtle one. It means the current
//! transform is differentiating inside an inner recursion, but the projection
//! points to an outer recursion group whose body has not been rewritten by this
//! transformer instance on the current descent path. In that situation the
//! outer group keeps its original slot numbering, so the primal projection is
//! forwarded unchanged and the tangent lanes are forced to zero.
//!
//! Conceptually:
//!
//! ```text
//! outer = rec([...])
//! inner = rec([ ..., proj(slot_i, ref(level = 2)), ... ])
//! ```
//!
//! While differentiating `inner`, `proj(slot_i, ref(level = 2))` still means
//! "read the already-existing primal slot `slot_i` from `outer`". No
//! differentiated `(1 + N)` expansion is available there unless the transform
//! has also entered and rebuilt `outer` on this path.
//!
//! A placeholder is inserted into the cache before descending into the REC body
//! so back-edges can resolve while the final interleaved node is still being
//! rebuilt. The placeholder is strictly internal to that recursive descent.
//! It is never an externally visible semantic result:
//!
//! - it breaks the cycle while recursive slots are being interned,
//! - it is replaced by the finished interleaved `DEBRUIJNREC` before the
//!   recursive descent returns,
//! - every public output projects from the rebuilt recursion, not the
//!   placeholder shape.
//!
//! The `de_bruijn_to_sym` conversion is deferred to `signal_prepare`, where it
//! runs once over all process outputs through one `Converter` instance. This
//! guarantees that the same `DEBRUIJNREC` physical node maps to the same
//! `SYMREC` name in every primal and tangent lane.
//!
//! ## Pass-through helper nodes (`attach`, `enable`, `control`, `Output`)
//!
//! These carry a left (signal) and right (side-effect/control) operand. Only
//! the left operand's tangent is forwarded; the right operand is structurally
//! preserved with its tangent dropped. `Output` is similarly transparent and
//! differentiates only the wrapped signal.
//!
//! ## Bargraph outputs (`vbargraph`, `hbargraph`)
//!
//! Zero tangent for both. Bargraphs are metering outputs, not DSP signal paths.
//!
//! # Zero-tangent fallback boundary
//! Several signal families are intentionally outside the current
//! differentiable subset and therefore preserve the primal while emitting zero
//! tangents on every lane.
//!
//! | Family | Current treatment | Rationale |
//! |--------|-------------------|-----------|
//! | integer comparisons / bitwise / shifts | zero tangent | discrete integer semantics |
//! | `button`, `checkbox` | zero tangent | event-like / piecewise-constant control |
//! | `int_cast`, `bit_cast` | zero tangent | discontinuous or representation-level operation |
//! | unknown `FFun` / non-unary `FFun` | zero tangent | no trusted derivative rule in this pass |
//! | writable-table reads/writes, soundfiles, standalone waveforms, `Gen`, `PermVar`, `TempVar` | zero tangent | mutable or not yet modeled as differentiable primitives |
//! | unmatched / defensive fallback variants | zero tangent | preserve compilation robustness while keeping the primal |
//!
//! This is a support boundary, not a claim that the mathematical derivative is
//! literally zero in all cases. The invariant of the pass is narrower: when a
//! signal family has no explicit forward rule, compilation stays defined and
//! the primal is preserved instead of fabricating an unverified tangent.
//!
//! # Integration contract
//! This module is intentionally internal to `propagate`:
//! - `box_arity_typed(...)` reports expanded output arity for `fad(expr, seeds…)`,
//! - output expansion happens only after the wrapped box has already lowered to
//!   signal IR,
//! - seeds are user-supplied through `fad(expr, seed)`; the seed
//!   sub-expression is lowered to signals like any other box and its outputs
//!   become the seed list. `[autodiff:false]` metadata is not consulted.
//!
//! # Ordering invariant
//! Tangent outputs are emitted deterministically:
//! 1. preserve primal output order,
//! 2. for each primal, emit one tangent per seed in the seed list's order.
//!
//! # Recursive code generation note
//! The unified multi-seed transform reuses the primal slot already present in
//! the differentiated recursive group. For recursive programs this removes the
//! previous "one AD-local primal shadow recursion per seed" pattern and makes
//! primal/tangent outputs project into the same interleaved `DEBRUIJNREC`.
//!
//! For example, the single-seed program
//!
//! ```text
//! process = fad((2 : + ~ *(p)), p);
//! ```
//!
//! now lowers conceptually to one recursive group:
//!
//! ```text
//! [ y[n] = p * y[n-1] + 2,
//!   dy/dp[n] = y[n-1] + p * dy/dp[n-1] ]
//! ```
//!
//! instead of:
//!
//! ```text
//! [ original primal recursion ] + [ duplicated AD-local primal recursion + tangent ]
//! ```
//!
//! The result is the same DSP semantics with less duplicated recursive state in
//! the emitted code.

use ahash::{AHashMap, AHashSet};
use signals::{BinOp, SigBuilder, SigId, SigMatch, match_sig};
use smallvec::SmallVec;
use tlib::{
    NodeKind, TreeArena, TreeId, check_de_bruijn_coherence, de_bruijn_rec, is_de_bruijn_closed,
    lift_de_bruijn, list_to_vec, match_de_bruijn_rec, match_de_bruijn_ref, tree_to_str,
    vec_to_list,
};

use crate::PropagateError;

/// Internal dual-number carrier used while differentiating one signal graph.
///
/// Carries the primal signal together with one tangent `SigId` per active seed
/// lane. Tangent order matches the enclosing transformer's `diff_seeds` vector.
/// The `SmallVec` inline capacity of 2 covers the common one- or two-seed
/// `fad` calls without spilling to the heap.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Dual {
    /// Original (undifferentiated) signal expression.
    primal: SigId,
    /// `tangents[j]` = derivative of `primal` with respect to `diff_seeds[j]`.
    tangents: SmallVec<[SigId; 2]>,
}

/// Memoized forward-mode transformer for one selected differentiation seed set.
///
/// A single instance is created per `fad(expr, seeds)` call and reused for
/// every primal output in `expr`. The cache prevents exponential blow-up on
/// reused subgraphs (DAG sharing) and breaks recursion cycles while rebuilding
/// interleaved `DEBRUIJNREC` groups (see module docs on the placeholder
/// insertion pattern).
///
/// Seed state is split into two complementary views:
///
/// - `diff_seeds` preserves caller-provided lane order (the order in which
///   tangents appear in the output bundle),
/// - `diff_seed_index` is the `SigId -> lane` reverse lookup used on every
///   visited node to short-circuit when the visited signal *is* a seed.
///
/// `debruijn_depth` counts only the `DEBRUIJNREC` scopes this transformer has
/// itself entered and rewritten on the current descent path. It drives the
/// `Proj` arm's classification of an enclosing group as already-rewritten
/// (interleaved `1+N` layout) versus unreachable-from-here (zero tangents).
struct ForwardADTransform<'a> {
    arena: &'a mut TreeArena,
    diff_seeds: Vec<SigId>,
    diff_seed_index: AHashMap<SigId, SmallVec<[usize; 2]>>,
    cache: AHashMap<SigId, Dual>,
    debruijn_depth: i64,
    /// First clock-boundary violation encountered during the sweep, if any.
    ///
    /// `transform` is infallible by design (it returns `Dual`s and relies on
    /// the cache for cycle-breaking), so boundary violations are recorded here
    /// and surfaced by [`generate_fad_signals_multi`] once the sweep finishes.
    /// Roadmap P0.4: a boundary must be a loud error, never a silent zero
    /// tangent.
    boundary_error: Option<PropagateError>,
    /// FAD Phase B (roadmap P5) "augment once" memo: original clocked-wrapper
    /// node (`OnDemand`/`Upsampling`/`Downsampling`) → its augmented twin whose
    /// payload carries the interleaved primal + tangent held-output lanes. Every
    /// `Seq(OD, y)` consumer routes through the *same* augmented block so a
    /// stateful body executes once per fire (cohabitation §6.1 "one block, not
    /// two").
    od_aug_cache: AHashMap<SigId, SigId>,
}

impl<'a> ForwardADTransform<'a> {
    fn new(arena: &'a mut TreeArena, diff_seeds: &[SigId]) -> Self {
        Self {
            arena,
            diff_seed_index: Self::build_seed_index(diff_seeds),
            diff_seeds: diff_seeds.to_vec(),
            cache: AHashMap::new(),
            debruijn_depth: 0,
            boundary_error: None,
            od_aug_cache: AHashMap::new(),
        }
    }

    /// Builds the reverse `SigId -> lane_indices` map used by seed recognition.
    ///
    /// A single `SigId` can appear on several lanes when the seed box lowers to
    /// a list containing repeated references (e.g. `fad(expr, (a, a))`); those
    /// lanes are intentionally preserved independently rather than being
    /// collapsed.
    fn build_seed_index(seeds: &[SigId]) -> AHashMap<SigId, SmallVec<[usize; 2]>> {
        let mut index = AHashMap::new();
        for (slot, &seed) in seeds.iter().enumerate() {
            index.entry(seed).or_insert_with(SmallVec::new).push(slot);
        }
        index
    }

    /// Number of active tangent lanes (one per seed).
    fn seed_count(&self) -> usize {
        self.diff_seeds.len()
    }

    /// Interleaved lane count per original recursion slot: `1` primal +
    /// `seed_count` tangents. Used as the multiplier for slot-index arithmetic
    /// in the `Proj` arm (`i * (1 + N) + 1 + lane`).
    fn bundle_lane_count(&self) -> i32 {
        1 + self.seed_count() as i32
    }

    fn repeat_lane_value(value: SigId, count: usize) -> SmallVec<[SigId; 2]> {
        let mut tangents = SmallVec::with_capacity(count);
        tangents.resize(count, value);
        tangents
    }

    fn repeated_lane_sig(&self, sig: SigId) -> SmallVec<[SigId; 2]> {
        Self::repeat_lane_value(sig, self.seed_count())
    }

    fn zero_tangent_lanes_real(&mut self) -> SmallVec<[SigId; 2]> {
        let zero = SigBuilder::new(self.arena).real(0.0);
        Self::repeat_lane_value(zero, self.seed_count())
    }

    fn zero_tangent_lanes_int(&mut self) -> SmallVec<[SigId; 2]> {
        let zero = SigBuilder::new(self.arena).int(0);
        Self::repeat_lane_value(zero, self.seed_count())
    }

    /// Differentiates one signal, using the shared DAG cache.
    ///
    /// Every visited `SigId` is cached so that multi-referenced subtrees are
    /// rewritten once. The cache doubles as a recursion-cycle breaker: the
    /// `DEBRUIJNREC` arm stores a self-referential placeholder before
    /// descending into the body, so back-edges resolve to a valid `Dual` while
    /// the final interleaved node is still being built (see module docs).
    fn transform(&mut self, sig: SigId) -> Dual {
        if let Some(dual) = self.cache.get(&sig).cloned() {
            debug_assert_eq!(
                dual.tangents.len(),
                self.seed_count(),
                "cached Dual has wrong tangent lane count"
            );
            return dual;
        }
        let depth_on_entry = self.debruijn_depth;
        let dual = self.transform_uncached(sig);
        debug_assert_eq!(
            self.debruijn_depth, depth_on_entry,
            "debruijn_depth not balanced across transform_uncached"
        );
        debug_assert_eq!(
            dual.tangents.len(),
            self.seed_count(),
            "transform_uncached produced wrong tangent lane count"
        );
        self.cache.insert(sig, dual.clone());
        dual
    }

    /// Differentiates each element of a signal cons-list and returns them as a
    /// `Vec<Dual>` preserving list order. Used to rebuild recursion-body
    /// element lists before re-interning them as a new cons-list.
    fn transform_list(&mut self, list: TreeId) -> Vec<Dual> {
        list_to_vec(self.arena, list)
            .unwrap_or_default()
            .into_iter()
            .map(|sig| self.transform(sig))
            .collect()
    }

    fn transform_uncached(&mut self, sig: SigId) -> Dual {
        if let Some(seed_slots) = self.diff_seed_index.get(&sig).cloned() {
            let one = SigBuilder::new(self.arena).real(1.0);
            let mut tangents = self.zero_tangent_lanes_real();
            for slot in seed_slots {
                tangents[slot] = one;
            }
            return Dual {
                primal: sig,
                tangents,
            };
        }

        if match_de_bruijn_ref(self.arena, sig).is_some() {
            return Dual {
                primal: sig,
                tangents: self.repeated_lane_sig(sig),
            };
        }

        if let Some(body) = match_de_bruijn_rec(self.arena, sig) {
            // Snapshot the set of SigIds already in the cache before entering
            // this recursive body.  Every entry added during body traversal is
            // computed under a *lifted-seed* context (seeds shifted by one De
            // Bruijn level) and is therefore WRONG in the enclosing scope.
            // Concretely: `SIGDELAY1(SIGPROJ(0, DEBRUIJNREF(1)))` appears as a
            // back-edge inside every single-slot feedback body *and* may be the
            // FAD seed itself (e.g. `prev_gain` in `fad(loss, prev_gain)`).
            // Without the cache snapshot the body-traversal entry wins and the
            // seed check never fires at the outer scope, producing a tangent of
            // `delay1(proj(1, ref))` instead of `1.0`.
            let outer_cache_keys: AHashSet<SigId> = self.cache.keys().copied().collect();

            // Pre-seed the cache with a self-referential placeholder so any
            // `DEBRUIJNREF(1)` back-edge discovered while differentiating the
            // body resolves to something shaped like a `Dual` instead of
            // triggering infinite recursion. This placeholder's tangents
            // point back at the original (un-rewritten) group; they are never
            // observed externally because the entry below is overwritten once
            // the rebuilt interleaved group is available.
            self.cache.insert(
                sig,
                Dual {
                    primal: sig,
                    tangents: self.repeated_lane_sig(sig),
                },
            );

            self.debruijn_depth += 1;

            // Crossing a new `DEBRUIJNREC` binder shifts every free
            // `DEBRUIJNREF` in the seed expressions by one level so that
            // `SigId` equality keeps firing when the same seed appears inside
            // the lifted body. The old seed state is stashed and restored on
            // exit so sibling subtrees (and nested RECs deeper still) start
            // from the correct snapshot.
            let old_seeds = std::mem::take(&mut self.diff_seeds);
            let old_seed_index = std::mem::replace(&mut self.diff_seed_index, AHashMap::new());
            self.diff_seeds = old_seeds
                .iter()
                .map(|&seed| lift_de_bruijn(self.arena, seed))
                .collect();
            self.diff_seed_index = Self::build_seed_index(&self.diff_seeds);

            let duals = self.transform_list(body);
            let original_arity = duals.len();

            self.diff_seeds = old_seeds;
            self.diff_seed_index = old_seed_index;
            self.debruijn_depth -= 1;

            // Discard all cache entries that were added while traversing the
            // body.  They used the lifted-seed index and are invalid at this
            // outer scope.  The only exception is `sig` itself (the current
            // `DEBRUIJNREC` node): its final dual is inserted below after the
            // expanded group is built.
            self.cache.retain(|k, _| outer_cache_keys.contains(k));

            // Interleave `[primal, tangent_s0, …, tangent_s{N-1}]` for every
            // original slot in source order. Downstream `Proj` nodes rely on
            // this exact `i * (1 + N) + (0 or 1 + lane)` layout.
            let lane_count = self.bundle_lane_count() as usize;
            let mut expanded_body = Vec::with_capacity(original_arity * lane_count);
            for dual in duals {
                debug_assert_eq!(
                    dual.tangents.len(),
                    self.seed_count(),
                    "REC body element has wrong tangent lane count"
                );
                expanded_body.push(dual.primal);
                expanded_body.extend(dual.tangents);
            }
            debug_assert_eq!(
                expanded_body.len(),
                original_arity * lane_count,
                "interleaved REC body length must equal k * (1 + N)"
            );
            let list_node = vec_to_list(self.arena, &expanded_body);
            let fad_rec = de_bruijn_rec(self.arena, list_node);
            // The whole rebuilt group IS both the primal and every tangent
            // "lane" at the group level; tangent selection only makes sense
            // once a `Proj` picks a specific slot out of the interleaved body.
            return Dual {
                primal: fad_rec,
                tangents: self.repeated_lane_sig(fad_rec),
            };
        }

        match match_sig(self.arena, sig) {
            SigMatch::Int(_) => Dual {
                primal: sig,
                tangents: self.zero_tangent_lanes_int(),
            },
            SigMatch::Real(_) => self.zero_tangent(sig),
            SigMatch::Input(_) => self.zero_tangent(sig),
            SigMatch::HSlider(_) | SigMatch::VSlider(_) | SigMatch::NumEntry(_) => {
                self.zero_tangent(sig)
            }
            SigMatch::Button(_) | SigMatch::Checkbox(_) => self.zero_tangent(sig),
            SigMatch::BinOp(op, x, y) => self.transform_binop(op, x, y),
            SigMatch::Sin(x) => self.unary_chain(
                x,
                |b, primal_x| b.sin(primal_x),
                |b, primal_x, tangent_x| {
                    let cos_x = b.cos(primal_x);
                    b.mul(cos_x, tangent_x)
                },
            ),
            SigMatch::Cos(x) => self.unary_chain(
                x,
                |b, primal_x| b.cos(primal_x),
                |b, primal_x, tangent_x| {
                    let sin_x = b.sin(primal_x);
                    let zero = b.real(0.0);
                    let neg_sin_x = b.sub(zero, sin_x);
                    b.mul(neg_sin_x, tangent_x)
                },
            ),
            SigMatch::Tan(x) => self.unary_chain(
                x,
                |b, primal_x| b.tan(primal_x),
                |b, primal_x, tangent_x| {
                    let cos_x = b.cos(primal_x);
                    let cos_sq = b.mul(cos_x, cos_x);
                    let one = b.real(1.0);
                    let inv = b.div(one, cos_sq);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Exp(x) => self.unary_chain(
                x,
                |b, primal_x| b.exp(primal_x),
                |b, primal_x, tangent_x| {
                    let exp_x = b.exp(primal_x);
                    b.mul(exp_x, tangent_x)
                },
            ),
            SigMatch::Exp10(x) => self.unary_chain(
                x,
                |b, primal_x| b.exp10(primal_x),
                |b, primal_x, tangent_x| {
                    let exp10_x = b.exp10(primal_x);
                    let ten = b.real(10.0);
                    let log_ten = b.log(ten);
                    let scale = b.mul(exp10_x, log_ten);
                    b.mul(scale, tangent_x)
                },
            ),
            SigMatch::Log(x) => self.unary_chain(
                x,
                |b, primal_x| b.log(primal_x),
                |b, primal_x, tangent_x| {
                    let one = b.real(1.0);
                    let inv = b.div(one, primal_x);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Log10(x) => self.unary_chain(
                x,
                |b, primal_x| b.log10(primal_x),
                |b, primal_x, tangent_x| {
                    let ten = b.real(10.0);
                    let log_ten = b.log(ten);
                    let denom = b.mul(primal_x, log_ten);
                    let one = b.real(1.0);
                    let inv = b.div(one, denom);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Sqrt(x) => self.unary_chain(
                x,
                |b, primal_x| b.sqrt(primal_x),
                |b, primal_x, tangent_x| {
                    let two = b.real(2.0);
                    let root = b.sqrt(primal_x);
                    let denom = b.mul(two, root);
                    let one = b.real(1.0);
                    let inv = b.div(one, denom);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Abs(x) => self.unary_chain(
                x,
                |b, primal_x| b.abs(primal_x),
                |b, primal_x, tangent_x| {
                    let denom = b.abs(primal_x);
                    let sign = b.div(primal_x, denom);
                    b.mul(sign, tangent_x)
                },
            ),
            SigMatch::Acos(x) => self.unary_chain(
                x,
                |b, primal_x| b.acos(primal_x),
                |b, primal_x, tangent_x| {
                    let one = b.real(1.0);
                    let x_sq = b.mul(primal_x, primal_x);
                    let inside = b.sub(one, x_sq);
                    let root = b.sqrt(inside);
                    let minus_one = b.real(-1.0);
                    let inv = b.div(minus_one, root);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Asin(x) => self.unary_chain(
                x,
                |b, primal_x| b.asin(primal_x),
                |b, primal_x, tangent_x| {
                    let one = b.real(1.0);
                    let x_sq = b.mul(primal_x, primal_x);
                    let inside = b.sub(one, x_sq);
                    let root = b.sqrt(inside);
                    let inv = b.div(one, root);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Atan(x) => self.unary_chain(
                x,
                |b, primal_x| b.atan(primal_x),
                |b, primal_x, tangent_x| {
                    let one = b.real(1.0);
                    let x_sq = b.mul(primal_x, primal_x);
                    let denom = b.add(one, x_sq);
                    let inv = b.div(one, denom);
                    b.mul(inv, tangent_x)
                },
            ),
            SigMatch::Pow(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.pow(dual_x.primal, dual_y.primal);
                let log_x = b.log(dual_x.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| {
                        let term1 = b.mul(ty, log_x);
                        let scaled_dx = b.mul(dual_y.primal, tx);
                        let term2 = b.div(scaled_dx, dual_x.primal);
                        let sum = b.add(term1, term2);
                        b.mul(primal, sum)
                    })
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Min(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.min(dual_x.primal, dual_y.primal);
                let cond = b.lt(dual_x.primal, dual_y.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| b.select2(cond, ty, tx))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Max(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.max(dual_x.primal, dual_y.primal);
                let cond = b.gt(dual_x.primal, dual_y.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| b.select2(cond, ty, tx))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Atan2(y, x) => {
                let dual_y = self.transform(y);
                let dual_x = self.transform(x);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.atan2(dual_y.primal, dual_x.primal);
                let x2 = b.mul(dual_x.primal, dual_x.primal);
                let y2 = b.mul(dual_y.primal, dual_y.primal);
                let denom = b.add(x2, y2);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| {
                        let x_dy = b.mul(dual_x.primal, ty);
                        let y_dx = b.mul(dual_y.primal, tx);
                        let num = b.sub(x_dy, y_dx);
                        b.div(num, denom)
                    })
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Fmod(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.fmod(dual_x.primal, dual_y.primal);
                let raw_q = b.div(dual_x.primal, dual_y.primal);
                let quotient = b.floor(raw_q);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| {
                        let scaled = b.mul(ty, quotient);
                        b.sub(tx, scaled)
                    })
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Remainder(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.remainder(dual_x.primal, dual_y.primal);
                let raw_q = b.div(dual_x.primal, dual_y.primal);
                let quotient = b.round(raw_q);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| {
                        let scaled = b.mul(ty, quotient);
                        b.sub(tx, scaled)
                    })
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Delay1(x) => {
                let dual_x = self.transform(x);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.delay1(dual_x.primal);
                let tangents = dual_x
                    .tangents
                    .into_iter()
                    .map(|tx| b.delay1(tx))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Delay(x, d) => {
                let dual_x = self.transform(x);
                let dual_d = self.transform(d);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.delay(dual_x.primal, dual_d.primal);
                let delayed_primal = b.delay1(dual_x.primal);
                let time_gradient = b.sub(dual_x.primal, delayed_primal);
                let delayed_time_gradient = b.delay(time_gradient, dual_d.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_d.tangents.iter().copied())
                    .map(|(tx, td)| {
                        let term1 = b.delay(tx, dual_d.primal);
                        let scaled_delay = b.mul(td, delayed_time_gradient);
                        b.sub(term1, scaled_delay)
                    })
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::RdTbl(table, ridx) => self.transform_rdtbl(table, ridx),
            SigMatch::Select2(cond, x, y) => {
                let dual_cond = self.transform(cond);
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.select2(dual_cond.primal, dual_x.primal, dual_y.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| b.select2(dual_cond.primal, tx, ty))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::Prefix(x, y) => {
                let dual_x = self.transform(x);
                let dual_y = self.transform(y);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.prefix(dual_x.primal, dual_y.primal);
                let tangents = dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| b.prefix(tx, ty))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::FloatCast(x) => {
                let dual_x = self.transform(x);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.float_cast(dual_x.primal);
                let tangents = dual_x
                    .tangents
                    .into_iter()
                    .map(|tx| b.float_cast(tx))
                    .collect::<SmallVec<[SigId; 2]>>();
                Dual { primal, tangents }
            }
            SigMatch::IntCast(x) => {
                let dual_x = self.transform(x);
                let mut b = SigBuilder::new(self.arena);
                let primal = b.int_cast(dual_x.primal);
                let tangents = Self::repeat_lane_value(b.int(0), self.seed_count());
                Dual { primal, tangents }
            }
            SigMatch::Proj(index, group) => {
                let dual_group = self.transform(group);
                // Classify the group this projection targets:
                // - `BoundRec`: the enclosing recursion has been rewritten by
                //   this transformer (either directly, or reached through a
                //   `DEBRUIJNREF` whose level stays within the stack of RECs
                //   we have already entered). Slot arithmetic uses the
                //   interleaved `1 + N` layout.
                // - `UnboundRef`: a `DEBRUIJNREF` pointing at an outer RE
                //   this transformer never entered on the current path, so
                //   its body keeps the original slot numbering. Primal
                //   forwards unchanged; tangents are forced to zero.
                // - `Other`: defensive fallback for non-de-Bruijn shapes;
                //   propagate the projection lane-wise.
                enum GroupKind {
                    BoundRec,
                    UnboundRef,
                    Other,
                }
                let kind = if match_de_bruijn_rec(self.arena, group).is_some() {
                    GroupKind::BoundRec
                } else if let Some(level) = match_de_bruijn_ref(self.arena, group) {
                    if level <= self.debruijn_depth {
                        GroupKind::BoundRec
                    } else {
                        GroupKind::UnboundRef
                    }
                } else {
                    GroupKind::Other
                };

                let lane_count = self.bundle_lane_count();
                let mut b = SigBuilder::new(self.arena);
                match kind {
                    GroupKind::BoundRec => {
                        let primal = b.proj(index * lane_count, dual_group.primal);
                        let tangents = dual_group
                            .tangents
                            .iter()
                            .enumerate()
                            .map(|(slot, &group_lane)| {
                                b.proj(index * lane_count + 1 + slot as i32, group_lane)
                            })
                            .collect::<SmallVec<[SigId; 2]>>();
                        Dual { primal, tangents }
                    }
                    GroupKind::UnboundRef => {
                        let primal = b.proj(index, dual_group.primal);
                        let tangents = Self::repeat_lane_value(b.real(0.0), self.seed_count());
                        Dual { primal, tangents }
                    }
                    GroupKind::Other => {
                        let primal = b.proj(index, dual_group.primal);
                        let tangents = dual_group
                            .tangents
                            .into_iter()
                            .map(|group_lane| b.proj(index, group_lane))
                            .collect::<SmallVec<[SigId; 2]>>();
                        Dual { primal, tangents }
                    }
                }
            }
            SigMatch::Output(_, inner) => self.transform(inner),
            SigMatch::Attach(x, y) => self.pass_through_binary(x, y, |b, px, py| b.attach(px, py)),
            SigMatch::Enable(x, y) => self.pass_through_binary(x, y, |b, px, py| b.enable(px, py)),
            SigMatch::Control(x, y) => {
                self.pass_through_binary(x, y, |b, px, py| b.control(px, py))
            }
            SigMatch::VBargraph(_, inner) | SigMatch::HBargraph(_, inner) => {
                let _ = self.transform(inner);
                self.zero_tangent(sig)
            }
            SigMatch::FFun(ff, largs) => self.transform_ffun(sig, ff, largs),
            // ── FAD Phase B (roadmap P5), boundary wrappers ──
            // Differentiation commutes with every boundary operator as long as
            // the clock does not depend on the seed (cohabitation §5): the
            // clock/clock-env child is opaque and never traversed, the value
            // child carries the tangent. These pass-through rules are the
            // wrapper half of the §6 dual-rule table; the block-augmentation
            // half (`Seq`/`OnDemand`/`Upsampling`/`Downsampling` → `OD_aug`)
            // is still staged below.
            SigMatch::TempVar(u) => {
                self.unary_chain(u, |b, p| b.temp_var(p), |b, _p, tx| b.temp_var(tx))
            }
            SigMatch::PermVar(u) => {
                self.unary_chain(u, |b, p| b.perm_var(p), |b, _p, tx| b.perm_var(tx))
            }
            SigMatch::Clocked(c, u) => self.unary_chain(
                u,
                move |b, p| b.clocked(c, p),
                move |b, _p, tx| b.clocked(c, tx),
            ),
            SigMatch::ZeroPad(u, h) => self.unary_chain(
                u,
                move |b, p| b.zero_pad(p, h),
                move |b, _p, tx| b.zero_pad(tx, h),
            ),
            // ── Block augmentation (the P5 core) ──
            // `Seq(OD, y)` → `{ Seq(OD_aug, y), Seq(OD_aug, y') }` where
            // `OD_aug` interleaves the primal + tangent held-output lanes in the
            // block payload, built once per source block (§6). The wrapper glue
            // inside `y` (`PermVar`/`Clocked`/`TempVar`) is handled by the rules
            // above; this arm augments the block itself and routes the value.
            SigMatch::Seq(od, y) => self.transform_seq(od, y),
            // A bare wrapper node reached *outside* a `Seq` is not a shape the
            // clock-domain lowering emits; keep it a loud error rather than a
            // silent zero tangent (roadmap P0.4, cohabitation §4).
            SigMatch::ClockEnvToken(_)
            | SigMatch::OnDemand(_)
            | SigMatch::Upsampling(_)
            | SigMatch::Downsampling(_) => {
                let kind = match match_sig(self.arena, sig) {
                    SigMatch::ClockEnvToken(_) => "clock-env token",
                    SigMatch::OnDemand(_) => "ondemand",
                    SigMatch::Upsampling(_) => "upsampling",
                    _ => "downsampling",
                };
                if self.boundary_error.is_none() {
                    self.boundary_error =
                        Some(PropagateError::FadUnsupportedNode { node: sig, kind });
                }
                // Keep the sweep total: the placeholder Dual is never
                // observable because the entry point fails afterwards.
                self.zero_tangent(sig)
            }
            _ => self.zero_tangent(sig),
        }
    }

    /// Returns `true` when the `FFUN` descriptor's function name (in any of
    /// its precision variants `f32` / `f64` / `long double`) matches one of
    /// the provided targets. This is how `tanhf` / `tanh` / `tanhl` are
    /// recognized as the same mathematical operation.
    fn ffun_is(arena: &TreeArena, ff: SigId, targets: &[&str]) -> bool {
        let Some(node) = arena.node(ff) else {
            return false;
        };
        let NodeKind::Tag(tag_id) = node.kind else {
            return false;
        };
        if arena.tag_name(tag_id) != Some("FFUN") {
            return false;
        }
        let [signature, _, _] = node.children.as_slice() else {
            return false;
        };
        let Some(sig_items) = list_to_vec(arena, *signature) else {
            return false;
        };
        let Some(names_node) = sig_items.get(1) else {
            return false;
        };
        let Some(name_ids) = list_to_vec(arena, *names_node) else {
            return false;
        };
        name_ids
            .iter()
            .any(|id| tree_to_str(arena, *id).is_some_and(|n| targets.contains(&n)))
    }

    /// Zero-tangent fallback: preserve the primal and emit a zero (real)
    /// tangent on every lane. See the module docstring's "Zero-tangent
    /// fallback boundary" table for the signal families that currently land
    /// here.
    fn zero_tangent(&mut self, sig: SigId) -> Dual {
        Dual {
            primal: sig,
            tangents: self.zero_tangent_lanes_real(),
        }
    }

    /// Classifies a table source as read-only (differentiable through the
    /// read index) when it is either:
    /// - `SIGWAVEFORM(...)`, or
    /// - `SIGWRTBL(size, generator, nil, nil)` — a write-once table used as a
    ///   read-only backing store (nil write index and nil write signal).
    ///
    /// Mutable tables (non-nil write ports) stay outside the differentiable
    /// subset and fall back to a zero tangent.
    fn is_readonly_table_source(&self, sig: SigId) -> bool {
        match match_sig(self.arena, sig) {
            SigMatch::Waveform(_) => true,
            SigMatch::WrTbl(_, _, widx, wsig) => self.arena.is_nil(widx) && self.arena.is_nil(wsig),
            _ => false,
        }
    }

    /// Differentiates `rdtbl(T, i)` for a read-only table `T`:
    ///
    /// ```text
    /// y  = rdtbl(T, i)
    /// y' = slope(i) · i'
    /// slope(i) ≈ (rdtbl(T, i + 1) - rdtbl(T, i - 1)) / 2
    /// ```
    ///
    /// The slope is a symmetric finite difference around `i`. Bounds
    /// behaviour for `i ± 1` is inherited from the existing runtime table
    /// read semantics — FAD deliberately does not invent extra wrap / clamp
    /// rules here.
    ///
    /// When `T` is not a read-only source, the primal is rebuilt but every
    /// tangent lane is forced to zero. Table *contents* are never
    /// differentiated.
    fn transform_rdtbl(&mut self, table: SigId, ridx: SigId) -> Dual {
        let dual_index = self.transform(ridx);
        let readonly = self.is_readonly_table_source(table);
        let mut b = SigBuilder::new(self.arena);
        let primal = b.rdtbl(table, dual_index.primal);
        if !readonly {
            return Dual {
                primal,
                tangents: self.zero_tangent_lanes_real(),
            };
        }

        let one = b.int(1);
        let two = b.real(2.0);
        let idx_plus = b.add(dual_index.primal, one);
        let idx_minus = b.sub(dual_index.primal, one);
        let plus = b.rdtbl(table, idx_plus);
        let minus = b.rdtbl(table, idx_minus);
        let diff = b.sub(plus, minus);
        let slope = b.div(diff, two);
        let tangents = dual_index
            .tangents
            .into_iter()
            .map(|tidx| b.mul(slope, tidx))
            .collect::<SmallVec<[SigId; 2]>>();
        Dual { primal, tangents }
    }

    /// Chain-rule helper for unary foreign functions.
    ///
    /// Builds `primal = ffun(arg.primal)` and then, for every tangent lane,
    /// calls `tangent_fn(builder, primal, arg.primal, arg.tangent_lane)` so
    /// the rule can reuse the primal output where it is cheaper (e.g. `tanh'
    /// = 1 − tanh²`, expressed directly from the primal).
    fn ffun_unary_chain<FTangent>(
        &mut self,
        ff: SigId,
        arg: SigId,
        mut tangent_fn: FTangent,
    ) -> Dual
    where
        FTangent: FnMut(&mut SigBuilder<'_>, SigId, SigId, SigId) -> SigId,
    {
        let dual_x = self.transform(arg);
        let largs = vec_to_list(self.arena, &[dual_x.primal]);
        let mut b = SigBuilder::new(self.arena);
        let primal = b.ffun(ff, largs);
        let tangents = dual_x
            .tangents
            .into_iter()
            .map(|tx| tangent_fn(&mut b, primal, dual_x.primal, tx))
            .collect::<SmallVec<[SigId; 2]>>();
        Dual { primal, tangents }
    }

    /// Dispatches differentiation of a foreign function (`FFUN`) call by
    /// matching the descriptor's name against the set of hand-rolled unary
    /// rules (hyperbolic and inverse-hyperbolic trig). Non-unary calls and
    /// unrecognized names fall back to a zero tangent with the primal
    /// unchanged, matching the module-level zero-tangent boundary.
    fn transform_ffun(&mut self, sig: SigId, ff: SigId, largs: SigId) -> Dual {
        let args = list_to_vec(self.arena, largs).unwrap_or_default();
        if args.len() != 1 {
            return self.zero_tangent(sig);
        }
        let arg = args[0];
        if Self::ffun_is(self.arena, ff, &["tanhf", "tanh", "tanhl"]) {
            self.ffun_unary_chain(ff, arg, |b, primal, _x, tx| {
                let tanh_sq = b.mul(primal, primal);
                let one = b.real(1.0);
                let sech_sq = b.sub(one, tanh_sq);
                b.mul(sech_sq, tx)
            })
        } else if Self::ffun_is(self.arena, ff, &["sinhf", "sinh", "sinhl"]) {
            self.ffun_unary_chain(ff, arg, |b, primal, _x, tx| {
                let sinh_sq = b.mul(primal, primal);
                let one = b.real(1.0);
                let one_plus_sq = b.add(one, sinh_sq);
                let cosh_x = b.sqrt(one_plus_sq);
                b.mul(cosh_x, tx)
            })
        } else if Self::ffun_is(self.arena, ff, &["coshf", "cosh", "coshl"]) {
            self.ffun_unary_chain(ff, arg, |b, _primal, x, tx| {
                let exp_x = b.exp(x);
                let minus_one = b.real(-1.0);
                let neg_x = b.mul(minus_one, x);
                let exp_neg_x = b.exp(neg_x);
                let diff = b.sub(exp_x, exp_neg_x);
                let half = b.real(0.5);
                let sinh_x = b.mul(half, diff);
                b.mul(sinh_x, tx)
            })
        } else if Self::ffun_is(self.arena, ff, &["atanhf", "atanh", "atanhl"]) {
            self.ffun_unary_chain(ff, arg, |b, _primal, x, tx| {
                let x_sq = b.mul(x, x);
                let one = b.real(1.0);
                let denom = b.sub(one, x_sq);
                b.div(tx, denom)
            })
        } else if Self::ffun_is(self.arena, ff, &["asinhf", "asinh", "asinhl"]) {
            self.ffun_unary_chain(ff, arg, |b, _primal, x, tx| {
                let x_sq = b.mul(x, x);
                let one = b.real(1.0);
                let sum = b.add(one, x_sq);
                let denom = b.sqrt(sum);
                b.div(tx, denom)
            })
        } else if Self::ffun_is(self.arena, ff, &["acoshf", "acosh", "acoshl"]) {
            self.ffun_unary_chain(ff, arg, |b, _primal, x, tx| {
                let x_sq = b.mul(x, x);
                let one = b.real(1.0);
                let diff = b.sub(x_sq, one);
                let denom = b.sqrt(diff);
                b.div(tx, denom)
            })
        } else {
            self.zero_tangent(sig)
        }
    }

    /// Chain-rule helper for native unary operators (`sin`, `log`, `sqrt`, …).
    ///
    /// `primal_fn(builder, primal_x) -> primal` rebuilds the outer operation.
    /// `tangent_fn(builder, primal_x, tangent_x_lane) -> tangent_lane` produces
    /// one tangent lane given the primal sub-expression and the tangent lane
    /// of the input; it is invoked once per active seed lane.
    fn unary_chain<FPrimal, FTangent>(
        &mut self,
        x: SigId,
        primal_fn: FPrimal,
        mut tangent_fn: FTangent,
    ) -> Dual
    where
        FPrimal: FnOnce(&mut SigBuilder<'_>, SigId) -> SigId,
        FTangent: FnMut(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
    {
        let dual_x = self.transform(x);
        let mut b = SigBuilder::new(self.arena);
        let primal = primal_fn(&mut b, dual_x.primal);
        let tangents = dual_x
            .tangents
            .into_iter()
            .map(|tx| tangent_fn(&mut b, dual_x.primal, tx))
            .collect::<SmallVec<[SigId; 2]>>();
        Dual { primal, tangents }
    }

    /// Transparent helper for `attach` / `enable` / `control`: the left
    /// operand carries the signal lane, the right operand is a
    /// side-effect/control reference that is structurally preserved but whose
    /// tangent is dropped. Both operands are still visited so seed
    /// recognition and caching stay consistent across the DAG.
    fn pass_through_binary<F>(&mut self, x: SigId, y: SigId, primal_fn: F) -> Dual
    where
        F: FnOnce(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
    {
        let dual_x = self.transform(x);
        let dual_y = self.transform(y);
        let mut b = SigBuilder::new(self.arena);
        Dual {
            primal: primal_fn(&mut b, dual_x.primal, dual_y.primal),
            tangents: dual_x.tangents,
        }
    }

    /// FAD Phase B (roadmap P5): differentiates `Seq(OD, y)`, the clock-domain
    /// wrapper's per-output node. The block `OD` is augmented **once** so its
    /// payload also carries the tangent held-outputs, and both the primal value
    /// `y` and each tangent lane `y'` are re-sequenced through the *same*
    /// augmented block (cohabitation §6, "one block, not two").
    fn transform_seq(&mut self, od: SigId, y: SigId) -> Dual {
        let Some(od_aug) = self.augment_block(od) else {
            if self.boundary_error.is_none() {
                self.boundary_error = Some(PropagateError::FadUnsupportedNode {
                    node: od,
                    kind: "clocked sequencing (Seq) around a non-wrapper block",
                });
            }
            // Placeholder primal; the entry point fails on `boundary_error`
            // before any dual is observable.
            let seq = SigBuilder::new(self.arena).seq(od, y);
            return self.zero_tangent(seq);
        };
        let y_dual = self.transform(y);
        let mut b = SigBuilder::new(self.arena);
        let primal = b.seq(od_aug, y_dual.primal);
        let tangents = y_dual
            .tangents
            .into_iter()
            .map(|t| b.seq(od_aug, t))
            .collect::<SmallVec<[SigId; 2]>>();
        Dual { primal, tangents }
    }

    /// Augments a clocked-wrapper block (`OnDemand`/`Upsampling`/`Downsampling`)
    /// so its payload carries the interleaved `[primal, tangent₀, …]` held-output
    /// lanes. Memoized per source node so every `Seq` consumer shares one block.
    ///
    /// Payload layout: `[clock, lane₀, lane₀', …, lane₁, lane₁', …]` — the first
    /// child is the (opaque, never differentiated) clock; each subsequent held
    /// lane is expanded to its primal followed by one tangent per seed. Returns
    /// `None` if `od` is not a clocked wrapper.
    fn augment_block(&mut self, od: SigId) -> Option<SigId> {
        if let Some(&aug) = self.od_aug_cache.get(&od) {
            return Some(aug);
        }
        // `.to_vec()` releases the arena borrow before the mutable `transform`.
        let (kind, payload): (&str, Vec<SigId>) = match match_sig(self.arena, od) {
            SigMatch::OnDemand(lanes) => ("od", lanes.to_vec()),
            SigMatch::Upsampling(lanes) => ("us", lanes.to_vec()),
            SigMatch::Downsampling(lanes) => ("ds", lanes.to_vec()),
            _ => return None,
        };
        let (&clock, held) = payload.split_first()?;
        let mut new_payload =
            Vec::with_capacity(1 + held.len() * self.bundle_lane_count() as usize);
        new_payload.push(clock);
        for &lane in held {
            let d = self.transform(lane);
            new_payload.push(d.primal);
            new_payload.extend(d.tangents);
        }
        let aug = {
            let mut b = SigBuilder::new(self.arena);
            match kind {
                "od" => b.on_demand(&new_payload),
                "us" => b.upsampling(&new_payload),
                _ => b.downsampling(&new_payload),
            }
        };
        self.od_aug_cache.insert(od, aug);
        Some(aug)
    }

    /// Differentiates a binary operator.
    ///
    /// The primal is always rebuilt from the operands' primals. The tangent
    /// follows the rule table in the module docstring: arithmetic ops use the
    /// standard sum / difference / product / quotient / `rem` rules; shifts,
    /// bitwise ops, and comparisons contribute zero (integer / discrete).
    fn transform_binop(&mut self, op: BinOp, x: SigId, y: SigId) -> Dual {
        let dual_x = self.transform(x);
        let dual_y = self.transform(y);
        let mut b = SigBuilder::new(self.arena);
        let primal = match op {
            BinOp::Add => b.add(dual_x.primal, dual_y.primal),
            BinOp::Sub => b.sub(dual_x.primal, dual_y.primal),
            BinOp::Mul => b.mul(dual_x.primal, dual_y.primal),
            BinOp::Div => b.div(dual_x.primal, dual_y.primal),
            BinOp::Rem => b.rem(dual_x.primal, dual_y.primal),
            BinOp::Lsh => b.lsh(dual_x.primal, dual_y.primal),
            BinOp::ARsh => b.arsh(dual_x.primal, dual_y.primal),
            BinOp::LRsh => b.lrsh(dual_x.primal, dual_y.primal),
            BinOp::Gt => b.gt(dual_x.primal, dual_y.primal),
            BinOp::Lt => b.lt(dual_x.primal, dual_y.primal),
            BinOp::Ge => b.ge(dual_x.primal, dual_y.primal),
            BinOp::Le => b.le(dual_x.primal, dual_y.primal),
            BinOp::Eq => b.eq(dual_x.primal, dual_y.primal),
            BinOp::Ne => b.ne(dual_x.primal, dual_y.primal),
            BinOp::And => b.and(dual_x.primal, dual_y.primal),
            BinOp::Or => b.or(dual_x.primal, dual_y.primal),
            BinOp::Xor => b.xor(dual_x.primal, dual_y.primal),
        };

        let tangents = match op {
            BinOp::Add => dual_x
                .tangents
                .iter()
                .copied()
                .zip(dual_y.tangents.iter().copied())
                .map(|(tx, ty)| b.add(tx, ty))
                .collect::<SmallVec<[SigId; 2]>>(),
            BinOp::Sub => dual_x
                .tangents
                .iter()
                .copied()
                .zip(dual_y.tangents.iter().copied())
                .map(|(tx, ty)| b.sub(tx, ty))
                .collect::<SmallVec<[SigId; 2]>>(),
            BinOp::Mul => dual_x
                .tangents
                .iter()
                .copied()
                .zip(dual_y.tangents.iter().copied())
                .map(|(tx, ty)| {
                    let t1 = b.mul(tx, dual_y.primal);
                    let t2 = b.mul(dual_x.primal, ty);
                    b.add(t1, t2)
                })
                .collect::<SmallVec<[SigId; 2]>>(),
            BinOp::Div => dual_x
                .tangents
                .iter()
                .copied()
                .zip(dual_y.tangents.iter().copied())
                .map(|(tx, ty)| {
                    let t1 = b.mul(tx, dual_y.primal);
                    let t2 = b.mul(dual_x.primal, ty);
                    let num = b.sub(t1, t2);
                    let den = b.mul(dual_y.primal, dual_y.primal);
                    b.div(num, den)
                })
                .collect::<SmallVec<[SigId; 2]>>(),
            BinOp::Rem => {
                let x_div_y = b.div(dual_x.primal, dual_y.primal);
                let floor = b.floor(x_div_y);
                dual_x
                    .tangents
                    .iter()
                    .copied()
                    .zip(dual_y.tangents.iter().copied())
                    .map(|(tx, ty)| {
                        let term2 = b.mul(ty, floor);
                        b.sub(tx, term2)
                    })
                    .collect::<SmallVec<[SigId; 2]>>()
            }
            BinOp::Lsh
            | BinOp::ARsh
            | BinOp::LRsh
            | BinOp::Gt
            | BinOp::Lt
            | BinOp::Ge
            | BinOp::Le
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::And
            | BinOp::Or
            | BinOp::Xor => Self::repeat_lane_value(b.int(0), self.seed_count()),
        };
        Dual { primal, tangents }
    }
}

/// Expands propagated primal outputs into the forward-mode AD output bundle.
///
/// Called by the `FlatNodeKind::ForwardAD` arm of `propagate` once the
/// wrapped primal and the seed sub-expression have both been lowered to
/// signals. One [`ForwardADTransform`] instance is allocated per call and
/// reused for every primal output so shared DAG subtrees are differentiated
/// at most once.
///
/// Output layout (per primal `p_i`, for `N = seeds.len()`):
///
/// ```text
/// [p_1, ∂p_1/∂s_0, …, ∂p_1/∂s_{N-1},
///  p_2, ∂p_2/∂s_0, …, ∂p_2/∂s_{N-1}, …]
/// ```
///
/// When `seeds` is empty the transform is a no-op and the primal outputs
/// pass through unchanged (keeps `fad(expr, ())` a legal identity).
pub(super) fn generate_fad_signals_multi(
    arena: &mut TreeArena,
    outputs: &[SigId],
    seeds: &[SigId],
) -> Result<Vec<SigId>, PropagateError> {
    if seeds.is_empty() {
        return Ok(outputs.to_vec());
    }

    let mut fad = ForwardADTransform::new(arena, seeds);
    let duals = outputs
        .iter()
        .map(|&sig| fad.transform(sig))
        .collect::<Vec<_>>();
    // Roadmap P0.4: surface the first clock-boundary violation recorded by
    // the sweep. This must happen before any dual is returned so no caller
    // can observe the silently-zero placeholder tangents.
    if let Some(err) = fad.boundary_error.take() {
        return Err(err);
    }

    let mut result = Vec::with_capacity(outputs.len() * (1 + seeds.len()));
    for dual in duals {
        result.push(dual.primal);
        result.extend(dual.tangents);
    }
    for &sig in &result {
        if is_de_bruijn_closed(arena, sig) {
            check_de_bruijn_coherence(arena, sig).map_err(|e| {
                PropagateError::DeBruijnCoherence {
                    pass: "FAD",
                    detail: e.to_string(),
                }
            })?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    //! Randomized structural invariants on the FAD rewriter.
    //!
    //! These tests build small random signal expressions — including nested
    //! `DEBRUIJNREC` groups with valid `DEBRUIJNREF` levels — and run
    //! [`ForwardADTransform`] over them. They do not check numeric
    //! correctness (that is handled by
    //! `crates/compiler/tests/fad_recursive_runtime.rs` through the
    //! interpreter); they validate the de Bruijn index bookkeeping. The
    //! `debug_assert!`s inside `transform` and the `(Rec)` arm fire on any
    //! layout violation, so a passing run on a wide variety of randomly
    //! generated expressions is a strong invariant check.
    //!
    //! No external proptest dependency: a small seeded LCG drives the
    //! generator deterministically.
    use super::*;
    use signals::SigBuilder;
    use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref, vec_to_list};

    /// Tiny linear-congruential PRNG for deterministic generation.
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
        }
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 32) as u32
        }
        fn gen_range(&mut self, lo: u32, hi_exclusive: u32) -> u32 {
            lo + (self.next_u32() % (hi_exclusive - lo))
        }
    }

    /// Builds a random closed signal expression of bounded depth.
    ///
    /// `depth_budget` caps recursion in the generator (not in the resulting
    /// tree). `rec_stack_arities[k]` is the arity of the `k+1`-th enclosing
    /// `DEBRUIJNREC` (innermost = index 0); used to emit *valid*
    /// `DEBRUIJNREF` levels and ensure every Proj on a free reference has a
    /// slot index within that group's body.
    fn gen_expr(
        rng: &mut Lcg,
        arena: &mut TreeArena,
        constants: &[SigId],
        depth_budget: u32,
        rec_stack_arities: &[usize],
    ) -> SigId {
        // Leaf weight rises as the budget shrinks.
        let force_leaf = depth_budget == 0 || rng.gen_range(0, 4) == 0;
        if force_leaf {
            // 50/50 between a constant and (when available) a Proj on an
            // enclosing recursion — that's how the (Proj-Bound /
            // Proj-Unbound) classification is exercised.
            if !rec_stack_arities.is_empty() && rng.gen_range(0, 2) == 0 {
                let level_idx = rng.gen_range(0, rec_stack_arities.len() as u32) as usize;
                let arity = rec_stack_arities[level_idx];
                let slot = rng.gen_range(0, arity as u32) as i32;
                let level = (level_idx + 1) as i64;
                let group = de_bruijn_ref(arena, level);
                let mut b = SigBuilder::new(arena);
                return b.proj(slot, group);
            }
            return constants[rng.gen_range(0, constants.len() as u32) as usize];
        }

        match rng.gen_range(0, 5) {
            // Binary +
            0 => {
                let x = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let y = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let mut b = SigBuilder::new(arena);
                b.add(x, y)
            }
            // Binary *
            1 => {
                let x = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let y = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let mut b = SigBuilder::new(arena);
                b.mul(x, y)
            }
            // sin
            2 => {
                let x = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let mut b = SigBuilder::new(arena);
                b.sin(x)
            }
            // delay1
            3 => {
                let x = gen_expr(rng, arena, constants, depth_budget - 1, rec_stack_arities);
                let mut b = SigBuilder::new(arena);
                b.delay1(x)
            }
            // DEBRUIJNREC of arity 1..=3, then Proj(0, …) on it
            _ => {
                let arity = rng.gen_range(1, 4) as usize;
                let mut new_stack = Vec::with_capacity(rec_stack_arities.len() + 1);
                new_stack.push(arity);
                new_stack.extend_from_slice(rec_stack_arities);
                let mut elements = Vec::with_capacity(arity);
                for _ in 0..arity {
                    elements.push(gen_expr(
                        rng,
                        arena,
                        constants,
                        depth_budget - 1,
                        &new_stack,
                    ));
                }
                let body = vec_to_list(arena, &elements);
                let rec = de_bruijn_rec(arena, body);
                let slot = rng.gen_range(0, arity as u32) as i32;
                let mut b = SigBuilder::new(arena);
                b.proj(slot, rec)
            }
        }
    }

    /// Smoke test: random expressions of varying depth and seed counts.
    /// The invariants checked here are the `debug_assert!`s embedded in
    /// `transform` and the `(Rec)` arm; this driver merely ensures they
    /// hold across a broad range of shapes.
    #[test]
    fn fad_invariants_hold_on_random_expressions() {
        const ITERS: u32 = 64;
        for run in 0..ITERS {
            let mut rng = Lcg::new(0xC0DE_FACE_u64 ^ u64::from(run));
            let mut arena = TreeArena::new();

            let c0 = SigBuilder::new(&mut arena).real(2.0);
            let c1 = SigBuilder::new(&mut arena).real(0.5);
            let s0 = SigBuilder::new(&mut arena).real(7.0);
            let s1 = SigBuilder::new(&mut arena).real(11.0);
            let constants = [c0, c1, s0, s1];

            let depth = rng.gen_range(2, 6);
            let expr = gen_expr(&mut rng, &mut arena, &constants, depth, &[]);

            // Vary seed configuration: 1 or 2 seeds, possibly with repeats.
            let seeds: Vec<SigId> = match rng.gen_range(0, 3) {
                0 => vec![s0],
                1 => vec![s0, s1],
                _ => vec![s0, s0],
            };
            let n = seeds.len();

            let result = generate_fad_signals_multi(&mut arena, &[expr], &seeds)
                .expect("FAD must succeed on closed random expressions");
            assert_eq!(
                result.len(),
                1 + n,
                "run {run}: output bundle must be [primal, t_s0, …, t_s{{N-1}}]"
            );
        }
    }

    /// Direct check: applying FAD to a seed itself yields primal == seed
    /// and a one-hot tangent on the matching lane.
    #[test]
    fn fad_of_seed_is_one_hot() {
        let mut arena = TreeArena::new();
        let s0 = SigBuilder::new(&mut arena).real(7.0);
        let s1 = SigBuilder::new(&mut arena).real(11.0);

        let result = generate_fad_signals_multi(&mut arena, &[s0, s1], &[s0, s1])
            .expect("FAD on seed list must succeed");
        // Layout: [s0, ds0/ds0, ds0/ds1, s1, ds1/ds0, ds1/ds1]
        assert_eq!(result.len(), 6);
        assert_eq!(result[0], s0);
        assert_eq!(result[3], s1);

        let one = SigBuilder::new(&mut arena).real(1.0);
        let zero = SigBuilder::new(&mut arena).real(0.0);
        assert_eq!(result[1], one, "ds0/ds0 must be 1.0");
        assert_eq!(result[2], zero, "ds0/ds1 must be 0.0");
        assert_eq!(result[4], zero, "ds1/ds0 must be 0.0");
        assert_eq!(result[5], one, "ds1/ds1 must be 1.0");
    }

    /// FAD Phase B (roadmap P5): the boundary *wrapper* rules commute with
    /// d/ds — `wrapper(s·x)' = wrapper(x)` — and must succeed (no
    /// FRS-PROP-0004). Only the value child is differentiated; the
    /// clock/clock-env child is opaque. The block-augmentation rules
    /// (`Seq`/`OnDemand`/…) are still staged and keep erroring.
    #[test]
    fn fad_phase_b_wrapper_boundaries_pass_through() {
        let mut arena = TreeArena::new();
        let s = SigBuilder::new(&mut arena).real(3.0);
        let x = SigBuilder::new(&mut arena).input(0);
        // A seed-dependent value so the tangent is non-trivial.
        let sx = SigBuilder::new(&mut arena).mul(s, x);

        // TempVar(u)' = TempVar(u')
        let tv = SigBuilder::new(&mut arena).temp_var(sx);
        let out = generate_fad_signals_multi(&mut arena, &[tv], &[s])
            .expect("FAD across a TempVar boundary must succeed");
        assert_eq!(out.len(), 2, "bundle is [primal, tangent]");
        assert!(matches!(match_sig(&arena, out[0]), SigMatch::TempVar(_)));
        assert!(
            matches!(match_sig(&arena, out[1]), SigMatch::TempVar(_)),
            "TempVar tangent must stay wrapped, not collapse to zero"
        );

        // PermVar(u)' = PermVar(u')
        let pv = SigBuilder::new(&mut arena).perm_var(sx);
        let out = generate_fad_signals_multi(&mut arena, &[pv], &[s])
            .expect("FAD across a PermVar boundary must succeed");
        assert!(matches!(match_sig(&arena, out[1]), SigMatch::PermVar(_)));

        // Clocked(c, u)' = Clocked(c, u') — the clock-env child is opaque.
        let env = SigBuilder::new(&mut arena).clock_env_token(0);
        let ck = SigBuilder::new(&mut arena).clocked(env, sx);
        let out = generate_fad_signals_multi(&mut arena, &[ck], &[s])
            .expect("FAD across a Clocked boundary must succeed");
        let SigMatch::Clocked(t_env, _) = match_sig(&arena, out[1]) else {
            panic!("Clocked tangent must stay a Clocked wrapper");
        };
        assert_eq!(t_env, env, "the clock-env child must be preserved verbatim");

        // ZeroPad(u, H)' = ZeroPad(u', H)
        let h = SigBuilder::new(&mut arena).real(1.0);
        let zp = SigBuilder::new(&mut arena).zero_pad(sx, h);
        let out = generate_fad_signals_multi(&mut arena, &[zp], &[s])
            .expect("FAD across a ZeroPad boundary must succeed");
        let SigMatch::ZeroPad(_, t_h) = match_sig(&arena, out[1]) else {
            panic!("ZeroPad tangent must stay a ZeroPad wrapper");
        };
        assert_eq!(t_h, h, "the clock/H child must be preserved verbatim");
    }

    /// Block augmentation is driven from `Seq(OD, y)` (the shape the clock-domain
    /// lowering emits). A **bare** `OnDemand` reached *outside* a `Seq` is not a
    /// shape lowering produces, so it stays a loud error (never a silent zero
    /// tangent) — the defensive guard.
    #[test]
    fn fad_phase_b_bare_wrapper_outside_seq_still_errors() {
        let mut arena = TreeArena::new();
        let s = SigBuilder::new(&mut arena).real(3.0);
        let x = SigBuilder::new(&mut arena).input(0);
        let sx = SigBuilder::new(&mut arena).mul(s, x);
        let env = SigBuilder::new(&mut arena).clock_env_token(0);
        let clock = SigBuilder::new(&mut arena).clocked(env, s);
        let od = SigBuilder::new(&mut arena).on_demand(&[clock, sx]);
        let err = generate_fad_signals_multi(&mut arena, &[od], &[s])
            .expect_err("bare OnDemand outside a Seq must error (defensive)");
        assert!(
            matches!(err, PropagateError::FadUnsupportedNode { .. }),
            "expected FadUnsupportedNode, got {err:?}"
        );
    }

    /// `Proj(0, REC([s, c]))` with seed `s` must rewrite to:
    ///   primal  = Proj(0 * (1+N), REC([…interleaved…]))
    ///   tangent = Proj(0 * (1+N) + 1, …)
    /// and the rebuilt REC body must have arity 2 * (1+N).
    #[test]
    fn fad_rec_proj_uses_interleaved_layout() {
        let mut arena = TreeArena::new();
        let s = SigBuilder::new(&mut arena).real(7.0);
        let c = SigBuilder::new(&mut arena).real(2.0);
        let body = vec_to_list(&mut arena, &[s, c]);
        let rec = de_bruijn_rec(&mut arena, body);
        let proj0 = SigBuilder::new(&mut arena).proj(0, rec);

        let result = generate_fad_signals_multi(&mut arena, &[proj0], &[s])
            .expect("FAD on Proj(REC) must succeed");
        assert_eq!(result.len(), 2);

        let primal = result[0];
        let tangent = result[1];

        let SigMatch::Proj(p_idx, p_group) = match_sig(&arena, primal) else {
            panic!("primal must be a Proj");
        };
        let SigMatch::Proj(t_idx, t_group) = match_sig(&arena, tangent) else {
            panic!("tangent must be a Proj");
        };

        // N = 1, so L = 2. Slot 0 -> primal index 0, tangent index 1.
        assert_eq!(p_idx, 0, "primal Proj index must be slot * L = 0");
        assert_eq!(t_idx, 1, "tangent Proj index must be slot * L + 1 = 1");
        assert_eq!(p_group, t_group, "both Projs must target the same REC");

        // Rebuilt REC body must have arity k * L = 2 * 2 = 4.
        let rebuilt_body =
            match_de_bruijn_rec(&arena, p_group).expect("primal Proj target must be a DEBRUIJNREC");
        let elems = list_to_vec(&arena, rebuilt_body).expect("REC body must be a list");
        assert_eq!(
            elems.len(),
            4,
            "interleaved body length must be k * (1 + N)"
        );
    }

    /// Regression: when the FAD seed signal structurally equals a back-edge
    /// reference inside a nested `DEBRUIJNREC` body, the transform must still
    /// yield tangent `1.0` for the seed at the outer scope.
    ///
    /// Setup:
    ///   `inner_rec = DEBRUIJNREC([k * delay1(proj(0, ref(1)))])`
    ///   `seed      = delay1(proj(0, DEBRUIJNREF(1)))`
    ///
    /// Because `inner_rec` has arity 1, its back-edge
    /// `delay1(proj(0, DEBRUIJNREF(1)))` hash-conses to the SAME `SigId` as
    /// `seed`.  Without cache scoping, the body-traversal entry for that
    /// SigId would shadow the outer-scope seed check, and `tangent(seed)`
    /// would return `delay1(proj(1,ref(1)))` instead of `real(1.0)`.
    ///
    /// We test with `expr = inner_out + seed` so the tangent is
    /// `tangent(inner_out) + tangent(seed)`.  Since `inner_rec` does not
    /// depend on `seed` at the outer scope, `tangent(inner_out)` is the
    /// tangent projection of the expanded rec and `tangent(seed)` must be
    /// `real(1.0)`.  An `Add(_, real(1.0))` tangent proves the seed check
    /// fired; anything else proves the bug is present.
    ///
    /// This corresponds to the `fad_gain1.dsp` bug where `prev_gain` (the FAD
    /// seed) is the delay-feedback of the outer loop, and the same expression
    /// appears as a back-edge in an inner noise `DEBRUIJNREC` body.
    #[test]
    fn fad_seed_not_poisoned_by_inner_rec_back_edge() {
        let mut arena = TreeArena::new();
        let k = SigBuilder::new(&mut arena).real(2.0);

        // inner_rec = DEBRUIJNREC([k * delay1(proj(0, DEBRUIJNREF(1)))])
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let back_proj = SigBuilder::new(&mut arena).proj(0, ref1);
        let back_delay = SigBuilder::new(&mut arena).delay1(back_proj);
        let inner_body_elem = SigBuilder::new(&mut arena).mul(k, back_delay);
        let inner_body_list = vec_to_list(&mut arena, &[inner_body_elem]);
        let inner_rec = de_bruijn_rec(&mut arena, inner_body_list);
        let inner_out = SigBuilder::new(&mut arena).proj(0, inner_rec);

        // seed = delay1(proj(0, DEBRUIJNREF(1))) — same SigId as `back_delay`
        let seed = back_delay;

        // expr = inner_out + seed
        // d(expr)/d(seed) = tangent(inner_out) + tangent(seed)
        //   tangent(inner_out) = proj(1, expanded_inner_rec) [zero at runtime]
        //   tangent(seed)      = 1.0                          [seed check]
        let expr = SigBuilder::new(&mut arena).add(inner_out, seed);

        let result = generate_fad_signals_multi(&mut arena, &[expr], &[seed])
            .expect("FAD must succeed on expr = inner_out + seed");
        assert_eq!(result.len(), 2, "one primal + one tangent lane");

        let tangent = result[1];

        // tangent must be Add(proj(1, fad_rec), real(1.0)).
        // Decompose the Add and check the second operand is real(1.0).
        let SigMatch::BinOp(op, _lhs, rhs) = match_sig(&arena, tangent) else {
            panic!(
                "tangent of (inner_out + seed) must be a BinOp; \
                 got a different node — seed check was poisoned by inner rec body cache"
            );
        };
        assert_eq!(op, signals::BinOp::Add, "tangent must be an Add");

        let one = SigBuilder::new(&mut arena).real(1.0);
        assert_eq!(
            rhs, one,
            "rhs of tangent Add must be real(1.0) — tangent(seed) must be 1.0, \
             not the body-scope tangent delay1(proj(1,ref(1)))"
        );
    }
}
