//! Infinite loop detector and per-pass evaluator caches.

use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tlib::TreeId;

use crate::SymId;
use crate::environment::{ClosureValue, EnvFrameKey, EvalCacheKey, EvalValue, PatternMatcherValue};
use crate::error::EvalError;

/// Infinite loop detector for recursive symbol expansion.
///
/// Detects two failure modes during evaluation:
/// 1. **Recursive loop**: a node is being evaluated while it is already on the call stack
///    (cyclic definition such as `x = x;`).
/// 2. **Depth exceeded**: the call stack grows beyond `max_depth`, indicating runaway recursion
///    in deeply nested but non-cyclic programs.
///
/// # C++ correspondence — `loopDetector.hh`
///
/// The C++ `LoopDetector` uses a `set<Tree>` to track in-flight nodes plus a recursion depth
/// counter. The Rust version uses a `Vec<TreeId>` for the call stack:
///
/// | Aspect | C++ (`LoopDetector`) | Rust (`LoopDetector`) |
/// |---|---|---|
/// | In-flight tracking | `set<Tree>` — O(log n) per check | `Vec<TreeId>` linear scan — O(n) per check |
/// | Depth counter | Separate `int` field | `call_stack.len()` |
/// | Check cost | O(log depth) tree-pointer comparison | O(depth) u32 comparison — cache-friendly |
///
/// For evaluation stacks typical of Faust programs (depth < 100), the Rust O(n) scan over a
/// compact vector is expected to be competitive with, and often faster than, the C++ O(log n)
/// set probe because the stack stays shallow and contiguous. The tree/set approach becomes more
/// attractive only when recursion depth grows far beyond the intended Faust range.
///
/// # Performance
/// - `enter`: O(depth) scan — the entire call stack fits in L1 cache for depth < 256.
/// - `leave`: O(1) — `Vec::pop`.
/// - Memory: 8 bytes per frame (one `u32` TreeId, padded).
///
/// # Evaluation-phase caches
///
/// `LoopDetector` is threaded through every recursive evaluator call, making it the
/// natural carrier for caches that must survive across the whole evaluation phase.
/// Currently it holds:
/// - `automaton_cache`: memoises the compiled `pattern_matcher::Automaton` for each
///   **evaluated** `Case` rule-list, keyed by the resulting rule-list `TreeId`.
///   This is important for parity: the same raw `case` syntax can yield different
///   effective patterns under different lexical environments.
#[derive(Clone, Debug)]
pub struct LoopDetector {
    pub(crate) call_stack: Vec<LoopFrame>,
    pub(crate) max_depth: usize,
    /// Structural lowering depth cap for this evaluator pass.
    ///
    /// This is read when the detector is constructed so one evaluation pass has
    /// stable recursion semantics even if the process environment changes later.
    pub(crate) structural_max_depth: usize,
    /// Cooperative cancellation flag.
    ///
    /// When set to `true`, the next `eval_value` call returns
    /// `EvalError::Cancelled`. This is the library-safe alternative to
    /// `process::exit`: the CLI sets this from a watchdog thread after the
    /// configured `--timeout`, and libfaust hosts can set it from any thread
    /// (e.g. on user abort).
    pub(crate) cancel: Arc<AtomicBool>,
    /// Compiled automata keyed by the `TreeId` of the evaluated `Case` rule-list.
    /// Definition-name map populated during eval: `box_id → definition_name`.
    ///
    /// When a named closure is forced to a box, the result `BoxId` is recorded
    /// here with the definition's string name. Consumed after eval by the SVG
    /// draw module to decorate / fold named sub-diagrams.
    ///
    /// C++ equivalent: `setDefNameProperty(result, id)` in `eval.cpp`.
    pub(crate) def_names: std::collections::HashMap<tlib::TreeId, String>,
    pub(crate) automaton_cache: crate::pattern_matcher::AutomatonCache,
    /// Dense store of `PatternMatcherValue` referenced by `boxPatternMatcher` nodes.
    ///
    /// Each `boxPatternMatcher` tree node carries a `boxInt(key)` child that
    /// indexes into this vector. The indirection is necessary because PM values
    /// contain environments and automatons that cannot be hash-consed.
    ///
    /// # C++ equivalent
    /// In C++, `boxPatternMatcher` inlines all PM state (automaton pointer,
    /// state index, environments, consumed args) in the tree. Rust keeps the
    /// complex data here and stores only a handle in the tree.
    pub(crate) pm_store: Vec<PatternMatcherValue>,
    /// Dense store of `ClosureValue` referenced by `boxClosure` nodes.
    ///
    /// Each `boxClosure` tree node carries a `boxInt(key)` child that indexes
    /// into this vector. This parallels `pm_store` for pattern matchers.
    ///
    /// # C++ equivalent
    /// In C++, `closure(expr, genv, visited, lenv)` is a tree node. Rust keeps
    /// the closure data here and stores only a handle in the tree.
    pub(crate) closure_store: Vec<ClosureValue>,
    /// Monotonic slot id source used by `a2sb` when lowering residual closures.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/eval.cpp`
    /// - `gGlobal->gBoxSlotNumber`
    ///
    /// The Rust port keeps this counter local to one evaluation pass instead of
    /// storing it in global state. The numeric payload is only used as a stable,
    /// debuggable slot label; semantic identity is carried by the unique `BoxId`
    /// of each `boxSlot(...)` node.
    pub(crate) next_slot_id: i32,
    /// Memoized `a2sb` lowering results keyed by original box identity.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/eval.cpp`
    /// - `gGlobal->gSymbolicBoxProperty`
    ///
    /// This cache preserves sharing when the same residual closure or pattern
    /// matcher appears multiple times in a diagram. Without it, each occurrence
    /// would allocate a fresh symbolic slot during lowering, changing arity and
    /// semantics for expressions such as `x-x` where both `x` uses must share
    /// one future input.
    pub(crate) symbolic_box_cache: ahash::HashMap<TreeId, TreeId>,
    /// Memoized evaluator results keyed by source expression and lexical environment.
    ///
    /// Source provenance (C++):
    /// - `compiler/evaluate/eval.cpp`
    /// - `getEvalProperty`
    /// - `setEvalProperty`
    ///
    /// Mapping status: `adapted`.
    /// The C++ evaluator stores this property directly on the tree arena. Rust
    /// keeps it inside the per-pass [`LoopDetector`] so one evaluation session
    /// preserves sharing without requiring mutable properties on tree nodes.
    pub(crate) eval_cache: ahash::HashMap<EvalCacheKey, EvalValue>,
    /// Structural recursion depth for `a2sb` / `a2sb_value` lowering.
    ///
    /// These paths create fresh slot nodes on every iteration, so the identity
    /// cache on `call_stack` never triggers. Without a separate counter, a
    /// legitimately diverging user program (mutual recursion that is not broken
    /// by `~`) overflows the OS stack and aborts the process. Incrementing this
    /// counter at each structural lowering entry lets us return
    /// `RecursionDepthExceeded` gracefully, matching the reference C++
    /// compiler's `"stack overflow in eval"` error.
    pub(crate) structural_depth: usize,
}

/// Environment variable overriding the default evaluator recursion budget.
const DEFAULT_EVAL_MAX_DEPTH_ENV: &str = "FAUST_RS_DEFAULT_EVAL_MAX_DEPTH";

/// Environment variable overriding the structural lowering recursion cap.
const STRUCTURAL_HARD_MAX_DEPTH_ENV: &str = "FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH";

/// Default fallback budget for identity-tracked evaluator recursion.
///
/// C++ detects evaluator stack overflow by watching the current stack address
/// and throws once it gets too close to the configured stack ceiling. Rust does
/// not have a portable thread-stack introspection API here, so normal evaluator
/// recursion is bounded by a logical frame count instead. This limit is applied
/// to [`LoopDetector::call_stack`], where frames carry stable tree/environment
/// identities and can therefore detect direct cycles as well as excessive
/// acyclic depth. Set [`DEFAULT_EVAL_MAX_DEPTH_ENV`] to a positive integer to
/// override this fallback for newly created detectors.
///
/// Raising it does not slow cycle detection: a genuine cycle is caught by frame
/// identity, not by exhausting the budget. The budget only bounds *acyclic*
/// runaway, so its only cost is how much real stack a runaway may consume
/// before it is reported.
///
/// # Choosing the value
///
/// # Why the value depends on the build profile
///
/// A logical frame does not cost a fixed amount of real stack: recursive `case`
/// evaluation puts several Rust frames on the stack per logical frame, and
/// debug builds make each of those far fatter. Measured 2026-08-06 on the
/// 64 MiB thread every faust-rs entry point spawns, using the same unbounded
/// `fact` recursion `diagnostic_errors` uses:
///
/// | build | last budget that reports cleanly | first that aborts | ≈ stack/frame |
/// |---|---|---|---|
/// | release | 16 384 | 32 768 | ~4 KiB |
/// | debug | 1 024 | 4 096 | ~64 KiB |
///
/// A single constant must therefore be sized for debug, which is what 1 024
/// was — and it is why raising it uniformly turns
/// `diagnostic_errors::diverging_recursive_case_reports_eval_error_instead_of_aborting`
/// from a passing test into a `SIGABRT` that takes the whole test binary with
/// it. That test is the guard for this constant; it also skips itself when the
/// *environment variable* raises the budget, which makes an env-var experiment
/// look like it passed when it merely opted out.
///
/// So the default follows the profile. Release keeps a factor of 2 against its
/// measured limit; debug keeps the value its own guard is sized for.
///
/// # What this buys
///
/// 1 024 rejected eight of the 297 DSPs in the reference `examples/` tree — all
/// finite-difference physical models — with `FRS-EVAL-0099` rather than any
/// real limitation. Seven need at most 4 096 and now compile.
/// `physicalModeling/fds/2dKirchhoffThinPlate.dsp` needs 32 768, past the
/// release limit above, so it stays a deliberate
/// `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH` opt-in rather than a default that trades a
/// clean diagnostic for a crash.
const DEFAULT_EVAL_MAX_DEPTH: usize = if cfg!(debug_assertions) { 1_024 } else { 8_192 };

/// Default fallback hard cap for structural lowering recursion.
///
/// Structural lowering (`a2sb` / `a2sb_value`) cannot use identity-based cycle
/// detection because some paths allocate fresh symbolic slots while descending.
/// It therefore keeps a separate depth counter and clamps even explicit
/// `with_max_depth(...)` requests to this value. Set
/// [`STRUCTURAL_HARD_MAX_DEPTH_ENV`] to a positive integer to override this
/// fallback for newly created detectors. Raising it opts into the risk that a
/// deeply recursive structural lowering path may overflow the real OS stack
/// before Rust can report `RecursionDepthExceeded`.
///
/// Kept at least [`DEFAULT_EVAL_MAX_DEPTH`]: this cap clamps even explicit
/// `with_max_depth` requests, so leaving it lower would silently cap a caller
/// that asked for more, which is how a raised evaluator budget would appear to
/// have no effect. It follows the profile for the same reason.
const STRUCTURAL_HARD_MAX_DEPTH: usize = if cfg!(debug_assertions) { 4_096 } else { 8_192 };

impl LoopDetector {
    /// Creates a detector with the default maximum recursion depth.
    ///
    /// The default is deliberately lower than the structural hard cap: recursive
    /// `case` evaluation can put several Rust frames on the real stack for each
    /// logical evaluator frame, so the default must trip before debug test
    /// threads overflow. Callers that know they are evaluating a deep acyclic
    /// program may opt into a higher budget with [`Self::with_max_depth`] or by
    /// setting `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH` before creating the detector.
    ///
    /// `FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH` controls the structural lowering cap
    /// used by `enter_structural`. Both environment variables must be
    /// positive decimal integers; invalid, missing, or zero values fall back to
    /// the compiled defaults.
    #[must_use]
    pub fn new() -> Self {
        let max_depth = depth_limit_from_env(DEFAULT_EVAL_MAX_DEPTH_ENV, DEFAULT_EVAL_MAX_DEPTH);
        Self::with_parts(max_depth, Arc::new(AtomicBool::new(false)))
    }

    /// Creates a detector with a pre-existing cooperative cancellation flag.
    ///
    /// The caller retains an `Arc<AtomicBool>` clone and can set it to `true`
    /// from any thread to request cancellation.  The next `eval_value` call
    /// will return `EvalError::Cancelled`.
    ///
    /// This is the library-safe alternative to `process::exit`: the CLI spawns
    /// a watchdog thread that sets the flag after `--timeout`, and libfaust
    /// hosts can set it on user abort without killing the process.
    #[must_use]
    pub fn with_cancel(cancel: Arc<AtomicBool>) -> Self {
        let max_depth = depth_limit_from_env(DEFAULT_EVAL_MAX_DEPTH_ENV, DEFAULT_EVAL_MAX_DEPTH);
        Self::with_parts(max_depth, cancel)
    }

    /// Creates a detector with an explicit maximum recursion depth.
    ///
    /// Use a lower value (e.g. 64) for unit tests that should never recurse
    /// deeply. Use a higher value for programs with known deep but non-cyclic
    /// definition chains. This explicit value is not read from
    /// `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH`; structural lowering is still clamped
    /// by `FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH` or its compiled fallback.
    #[must_use]
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self::with_parts(max_depth, Arc::new(AtomicBool::new(false)))
    }

    /// Shared constructor behind [`new`](Self::new), [`with_cancel`](Self::with_cancel)
    /// and [`with_max_depth`](Self::with_max_depth).
    ///
    /// Reads the structural hard-limit from the environment, then initializes all
    /// empty stacks, caches and side-tables from the given `max_depth` and
    /// `cancel` flag.
    fn with_parts(max_depth: usize, cancel: Arc<AtomicBool>) -> Self {
        let structural_max_depth =
            depth_limit_from_env(STRUCTURAL_HARD_MAX_DEPTH_ENV, STRUCTURAL_HARD_MAX_DEPTH);
        Self {
            call_stack: Vec::new(),
            max_depth,
            structural_max_depth,
            cancel,
            def_names: std::collections::HashMap::new(),
            automaton_cache: crate::pattern_matcher::AutomatonCache::new(),
            pm_store: Vec::new(),
            closure_store: Vec::new(),
            next_slot_id: 0,
            symbolic_box_cache: ahash::HashMap::with_hasher(ahash::RandomState::new()),
            eval_cache: ahash::HashMap::with_hasher(ahash::RandomState::new()),
            structural_depth: 0,
        }
    }

    /// Returns a clone of the cancellation flag for external threads to signal abort.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Returns `Err(EvalError::Cancelled)` if the cancel flag has been set.
    #[inline]
    pub(crate) fn check_cancel(&self) -> Result<(), EvalError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(EvalError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Stores a `PatternMatcherValue` and returns its dense key for `boxPatternMatcher` nodes.
    pub(crate) fn store_pm(&mut self, pm: PatternMatcherValue) -> i32 {
        let key = self.pm_store.len() as i32;
        self.pm_store.push(pm);
        key
    }

    /// Retrieves a stored `PatternMatcherValue` by cloning it out.
    ///
    /// Returns `None` if the key is out of range.
    pub(crate) fn get_pm(&self, key: i32) -> Option<PatternMatcherValue> {
        self.pm_store.get(key as usize).cloned()
    }

    /// Stores a `ClosureValue` and returns its dense key for `boxClosure` nodes.
    pub(crate) fn store_closure(&mut self, cv: ClosureValue) -> i32 {
        let key = self.closure_store.len() as i32;
        self.closure_store.push(cv);
        key
    }

    /// Retrieves a stored `ClosureValue` by cloning it out.
    ///
    /// Returns `None` if the key is out of range.
    pub(crate) fn get_closure(&self, key: i32) -> Option<ClosureValue> {
        self.closure_store.get(key as usize).cloned()
    }

    /// Pushes a frame keyed by `(tree, environment)` and checks for a cycle.
    ///
    /// Used when re-evaluating a tree node in a given environment; returns
    /// [`EvalError::LoopDetected`] if the same pair is already on the stack and
    /// [`EvalError::RecursionDepthExceeded`] past `max_depth`. Pair with
    /// [`leave`](Self::leave).
    pub(crate) fn enter_tree(&mut self, id: TreeId, env_key: EnvFrameKey) -> Result<(), EvalError> {
        self.enter(LoopFrame::TreeEnv { id, env_key }, id)
    }

    /// Pushes a frame keyed by `(symbol, environment)` and checks for a cycle.
    ///
    /// Used when resolving a definition; `node` is the tree blamed in a
    /// [`EvalError::LoopDetected`]. Pair with [`leave`](Self::leave).
    pub(crate) fn enter_symbol_env(
        &mut self,
        sym: SymId,
        env_key: EnvFrameKey,
        node: TreeId,
    ) -> Result<(), EvalError> {
        self.enter(LoopFrame::SymbolEnv { sym, env_key }, node)
    }

    /// Pushes `frame` after the cycle and depth checks shared by both `enter_*`
    /// methods.
    ///
    /// Returns [`EvalError::LoopDetected`] (blaming `node`) if `frame` is already
    /// on the stack, or [`EvalError::RecursionDepthExceeded`] once `max_depth` is
    /// reached.
    fn enter(&mut self, frame: LoopFrame, node: TreeId) -> Result<(), EvalError> {
        if self.call_stack.contains(&frame) {
            return Err(EvalError::LoopDetected { node });
        }
        if self.call_stack.len() >= self.max_depth {
            return Err(EvalError::RecursionDepthExceeded {
                max_depth: self.max_depth,
            });
        }
        self.call_stack.push(frame);
        Ok(())
    }

    /// Pops the most recently entered frame, balancing an `enter_*` call.
    pub(crate) fn leave(&mut self) {
        let _ = self.call_stack.pop();
    }

    /// Enters a structural lowering frame (`a2sb` / `a2sb_value`).
    ///
    /// Unlike [`enter`], this path does not record an identity key because
    /// every iteration creates a fresh `boxSlot`, making cycle detection
    /// impossible. The counter only enforces the `max_depth` budget so a
    /// diverging user program fails with `RecursionDepthExceeded` instead of
    /// aborting the process on OS stack overflow.
    pub(crate) fn enter_structural(&mut self) -> Result<(), EvalError> {
        let limit = self.max_depth.min(self.structural_max_depth);
        if self.structural_depth >= limit {
            return Err(EvalError::RecursionDepthExceeded { max_depth: limit });
        }
        self.structural_depth += 1;
        Ok(())
    }

    /// Decrements the structural-lowering counter, balancing an
    /// [`enter_structural`](Self::enter_structural) call.
    pub(crate) fn leave_structural(&mut self) {
        self.structural_depth = self.structural_depth.saturating_sub(1);
    }
}

/// Reads a recursion-depth limit from the environment variable `var_name`,
/// falling back to `fallback` when it is unset or invalid.
fn depth_limit_from_env(var_name: &str, fallback: usize) -> usize {
    depth_limit_from_env_value(std::env::var_os(var_name).as_deref(), fallback)
}

/// Pure core of [`depth_limit_from_env`], taking the raw value directly so it
/// can be unit-tested without touching process environment.
fn depth_limit_from_env_value(raw: Option<&OsStr>, fallback: usize) -> usize {
    raw.and_then(parse_depth_limit).unwrap_or(fallback)
}

/// Parses a depth limit, accepting only strictly positive integers.
///
/// Returns `None` for non-UTF-8, non-numeric, or zero values so the caller can
/// apply its fallback.
fn parse_depth_limit(raw: &OsStr) -> Option<usize> {
    let parsed = raw.to_str()?.trim().parse::<usize>().ok()?;
    (parsed > 0).then_some(parsed)
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// One recursion stack frame recorded by [`LoopDetector`].
pub(crate) enum LoopFrame {
    TreeEnv { id: TreeId, env_key: EnvFrameKey },
    SymbolEnv { sym: SymId, env_key: EnvFrameKey },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_depth_uses_default_eval_budget() {
        let mut detector = LoopDetector::new();
        for _ in 0..DEFAULT_EVAL_MAX_DEPTH {
            detector.enter_structural().unwrap();
        }

        assert!(matches!(
            detector.enter_structural(),
            Err(EvalError::RecursionDepthExceeded {
                max_depth: DEFAULT_EVAL_MAX_DEPTH
            })
        ));
    }

    #[test]
    fn structural_depth_clamps_explicitly_higher_eval_budget() {
        let mut detector = LoopDetector::with_max_depth(STRUCTURAL_HARD_MAX_DEPTH + 1);
        for _ in 0..STRUCTURAL_HARD_MAX_DEPTH {
            detector.enter_structural().unwrap();
        }

        assert!(matches!(
            detector.enter_structural(),
            Err(EvalError::RecursionDepthExceeded {
                max_depth: STRUCTURAL_HARD_MAX_DEPTH
            })
        ));
    }

    #[test]
    fn structural_depth_uses_detector_structural_budget() {
        let mut detector = LoopDetector::with_max_depth(10);
        detector.structural_max_depth = 3;

        for _ in 0..3 {
            detector.enter_structural().unwrap();
        }

        assert!(matches!(
            detector.enter_structural(),
            Err(EvalError::RecursionDepthExceeded { max_depth: 3 })
        ));
    }

    #[test]
    fn depth_limit_env_parser_accepts_positive_values() {
        assert_eq!(
            depth_limit_from_env_value(Some(OsStr::new(" 2048 ")), 64),
            2_048
        );
    }

    #[test]
    fn depth_limit_env_parser_rejects_missing_zero_or_invalid_values() {
        assert_eq!(depth_limit_from_env_value(None, 64), 64);
        assert_eq!(depth_limit_from_env_value(Some(OsStr::new("0")), 64), 64);
        assert_eq!(depth_limit_from_env_value(Some(OsStr::new("-1")), 64), 64);
        assert_eq!(
            depth_limit_from_env_value(Some(OsStr::new("not-a-depth")), 64),
            64
        );
    }
}
