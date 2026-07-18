//! Read-only feasibility analysis for stateful reverse-mode AD.
//!
//! # Source provenance
//! Original Rust design for RAD phase E0, documented in
//! `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md` section
//! "19. Feasibility analysis for stateful RAD".
//!
//! # Scope
//! This module does not enable reverse-mode differentiation through
//! recursive or delayed signals. It classifies one recursive signal group so
//! future RAD phases can decide whether an exact stateful adjoint is
//! structurally plausible and which implementation family it would need.
//!
//! Current phase-1 `rad(...)` lowering still rejects temporal and recursive
//! signal families in `reverse_ad`. The functions here are used only as a
//! gate for diagnostics and as a preparatory predicate for later phases:
//!
//! - phase E1 can attempt a block-local transpose for groups classified as
//!   [`RadRecLinearity::LinearLti`];
//! - phase E2 can replay primal-block coefficients for groups classified as
//!   [`RadRecLinearity::LinearTimeVarying`];
//! - phase F must use finite-horizon back-propagation through time (BPTT) for
//!   groups classified as [`RadRecLinearity::Nonlinear`].
//!
//! # Input representation
//! The analysis runs on the De Bruijn recursion form produced during
//! propagation, before `signal_prepare` converts recursive placeholders into
//! symbolic `sigRec`/`sigProj` names:
//!
//! ```text
//! DEBRUIJNREC([branch_0, branch_1, ...])
//! DEBRUIJNREF(1)             // reference to the current recursion group
//! Proj(i, DEBRUIJNREF(1))    // recursive state lane i
//! ```
//!
//! The `current_level` value tracks nested De Bruijn scopes. A reference at
//! the current level is treated as dependence on the state being classified;
//! references to any other level are treated as independent time-varying
//! signals because they are outside the current recurrence's state vector.
//!
//! # Classification model
//! Each signal node is summarized by three facts:
//!
//! - whether it depends on a current-recursion state lane;
//! - the worst linearity class seen along any current-state path;
//! - whether the part independent of the current state is a literal constant
//!   or a time-varying signal.
//!
//! These facts let the local rule table distinguish `0.5 * state` from
//! `input(0) * state` and from `state * state`, without expanding the signal
//! graph into algebraic normal form.
//!
//! # Result classes
//! A successful recursive-group classification has the following meaning:
//!
//! - [`RadRecLinearity::LinearLti`] means every current-recursion back-edge
//!   appears linearly and only under constant coefficients. Independent input
//!   or UI driving terms are allowed because they do not change the linearity
//!   of the state transition.
//! - [`RadRecLinearity::LinearTimeVarying`] means the current back-edge is
//!   still linear, but at least one coefficient depends on a non-constant
//!   signal such as an input or UI control.
//! - [`RadRecLinearity::Nonlinear`] means a current back-edge flows through a
//!   nonlinear primitive, a branch, a discrete cast, or another expression
//!   that is not affine in the recursive variables.
//!
//! The lattice is monotonic: `Nonlinear` dominates
//! `LinearTimeVarying`, which dominates `LinearLti`. Multi-output recursive
//! groups are classified by taking that maximum across every branch.
//!
//! # Conservative boundaries
//! The analysis is intentionally conservative and read-only. Unsupported or
//! opaque signal families are treated as time-varying coefficients unless
//! they contain the current recursive reference in a recognized nonlinear
//! position.
//!
//! The classifier accepts temporal shifts such as `delay1(state)` as
//! structurally linear. That does not mean phase-1 RAD can lower them: the
//! reverse transpose of a delay is anti-causal in stream time and needs the
//! future block/tape convention described in the RAD plan.
//!
//! # Complexity
//! Classification is linear in the visited signal DAG for a fixed De Bruijn
//! level. `LinearityAnalyzer` memoizes `(SigId, current_level)` because the
//! same shared subgraph can be visited from several recursive branches and
//! because nested recursive groups change the meaning of a De Bruijn level.

use ahash::AHashMap;
use signals::{BinOp, SigId, SigMatch, match_sig};
use tlib::{TreeArena, TreeId, list_to_vec, match_de_bruijn_rec, match_de_bruijn_ref};

/// Linearity class for a recursive group with respect to its own back-edges.
///
/// This is a structural predicate for future stateful RAD phases, not a
/// promise that `rad(...)` currently accepts the group.
///
/// # Ordering
/// The implicit severity order is:
///
/// ```text
/// LinearLti < LinearTimeVarying < Nonlinear
/// ```
///
/// `RadRecLinearity::max` implements that join operation internally when
/// combining branches or child expressions.
///
/// # What counts as "recursive"
/// Only references to the De Bruijn level currently being classified are
/// considered recursive-state dependence. Other recursion references can
/// still vary over time, but they are coefficients or driving signals from
/// this group's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadRecLinearity {
    /// Linear time-invariant state transition: recursive variables only appear
    /// linearly and under literal coefficients.
    ///
    /// A typical shape is `input + 0.5 * Proj(0, DEBRUIJNREF(1))`. The input
    /// term is allowed because it is independent of the recursive state; the
    /// back-edge coefficient is constant, so the state transition matrix is
    /// fixed across the block.
    LinearLti,
    /// Linear but time-varying state transition: recursive variables only
    /// appear linearly, but at least one coefficient is signal-dependent.
    ///
    /// A typical shape is `input(0) * Proj(0, DEBRUIJNREF(1))`. The state lane
    /// remains affine, but the coefficient must be replayed from the primal
    /// block at the matching sample for an exact reverse pass.
    LinearTimeVarying,
    /// Nonlinear dependence on a recursive variable.
    ///
    /// This covers `state * state`, `sin(state)`, state-dependent branches,
    /// state-dependent table indices, and discrete casts or comparisons that
    /// consume the current recursive state.
    Nonlinear,
}

/// Strategy gate for a future RAD pass over one recursive signal group.
///
/// The enum names mirror plan §19.6:
///
/// - [`RecRadMode::LinearTranspose`] is the E1 target. The group is eligible
///   for an exact block-local linear transpose, but `rad(...)` still needs the
///   surrounding block/tape evaluation convention before it can emit
///   user-visible code.
/// - [`RecRadMode::BlockLinearTimeVarying`] is the E2 target. The group stays
///   linear in recursive state, but coefficients must be read from the primal
///   block at the corresponding sample.
/// - [`RecRadMode::BpttRequired`] is the phase-F target for nonlinear
///   feedback; it requires finite-horizon unrolling and a backward sweep.
///
/// The conversion from [`RadRecLinearity`] is intentionally lossless at the
/// strategy level. This enum is a scheduling/diagnostic boundary; it does not
/// carry a horizon, tape policy, or backend allocation model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecRadMode {
    /// Phase E1 candidate: linear time-invariant recursive state transition.
    ///
    /// A future implementation can transpose the state matrix structurally and
    /// run the adjoint recurrence over a finite block in reverse sample order.
    LinearTranspose,
    /// Phase E2 candidate: linear time-varying transition requiring block
    /// coefficient replay.
    ///
    /// The recursive-state dependence is affine, but at least one coefficient
    /// is a signal. Exact reverse mode must read the coefficient value that the
    /// primal pass used at the corresponding sample.
    BlockLinearTimeVarying,
    /// Phase F candidate: nonlinear recurrence requiring BPTT.
    ///
    /// A local graph transpose is not enough because the current-state path
    /// crosses nonlinear or discrete control flow. A future implementation must
    /// unroll a horizon, record required primal intermediates, and sweep the
    /// unrolled graph backward.
    BpttRequired,
}

impl From<RadRecLinearity> for RecRadMode {
    /// Maps the structural linearity class to the first future RAD strategy
    /// capable of preserving exact reverse-mode semantics for that class.
    fn from(value: RadRecLinearity) -> Self {
        match value {
            RadRecLinearity::LinearLti => Self::LinearTranspose,
            RadRecLinearity::LinearTimeVarying => Self::BlockLinearTimeVarying,
            RadRecLinearity::Nonlinear => Self::BpttRequired,
        }
    }
}

impl RadRecLinearity {
    /// Returns the more conservative of two recursive-linearity classes.
    ///
    /// This is the join operation for the classifier lattice. It is used when
    /// combining independent branches, summing two recursive-state paths, or
    /// propagating a time-varying coefficient through an otherwise LTI term.
    fn max(self, other: Self) -> Self {
        use RadRecLinearity::{LinearLti, LinearTimeVarying, Nonlinear};
        match (self, other) {
            (Nonlinear, _) | (_, Nonlinear) => Nonlinear,
            (LinearTimeVarying, _) | (_, LinearTimeVarying) => LinearTimeVarying,
            (LinearLti, LinearLti) => LinearLti,
        }
    }

    /// Upgrades a state-dependent term when it is multiplied or divided by a
    /// state-independent coefficient.
    ///
    /// Constant coefficients preserve the existing class. Time-varying
    /// coefficients make the term at least
    /// [`RadRecLinearity::LinearTimeVarying`], while preserving a prior
    /// [`RadRecLinearity::Nonlinear`] result.
    fn with_coeff(self, variation: IndependentVariation) -> Self {
        match variation {
            IndependentVariation::Constant => self,
            IndependentVariation::TimeVarying => self.max(Self::LinearTimeVarying),
        }
    }
}

/// Variation class for subexpressions that are independent of the current
/// recursive state.
///
/// This intentionally has only two values. The classifier does not need to
/// know the exact expression for a coefficient; it only needs to know whether
/// that coefficient can change from sample to sample and therefore invalidates
/// the LTI transpose shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IndependentVariation {
    /// Literal integer/real constants and combinations proven constant by the
    /// local rule table.
    Constant,
    /// Inputs, UI controls, foreign variables, soundfiles, table reads, or any
    /// opaque state-independent expression that may vary across samples.
    TimeVarying,
}

impl IndependentVariation {
    /// Returns the more conservative variation class.
    fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::TimeVarying, _) | (_, Self::TimeVarying) => Self::TimeVarying,
            (Self::Constant, Self::Constant) => Self::Constant,
        }
    }
}

/// Local abstract value used while classifying one signal expression.
///
/// `ExprClass` is deliberately small: it is a product of the three facts the
/// RAD strategy decision needs. It does not preserve the shape of the signal
/// expression and it does not attempt symbolic simplification beyond the
/// structural matcher rules in [`LinearityAnalyzer::classify_uncached`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExprClass {
    /// Whether the expression reads `Proj(_, DEBRUIJNREF(current_level))` or a
    /// nested expression that depends on it.
    depends_on_current_rec: bool,
    /// Worst linearity class along paths that do depend on the current
    /// recursive state. For state-independent expressions this is always
    /// [`RadRecLinearity::LinearLti`] and should be ignored in favor of
    /// [`ExprClass::independent_variation`].
    rec_linearity: RadRecLinearity,
    /// Variation of the expression when it is independent of the current
    /// recursive state. For state-dependent expressions this is reset to
    /// [`IndependentVariation::Constant`] because coefficient variation is
    /// folded into [`ExprClass::rec_linearity`] at multiplication/division
    /// boundaries.
    independent_variation: IndependentVariation,
}

impl ExprClass {
    /// Class for a literal value or a composition proven constant by local
    /// rules.
    fn constant() -> Self {
        Self {
            depends_on_current_rec: false,
            rec_linearity: RadRecLinearity::LinearLti,
            independent_variation: IndependentVariation::Constant,
        }
    }

    /// Class for a state-independent expression that may vary across samples.
    ///
    /// This is the conservative fallback for inputs, controls, tables,
    /// soundfile access, foreign variables/functions, malformed nested lists,
    /// and unknown signal families.
    fn time_varying() -> Self {
        Self {
            depends_on_current_rec: false,
            rec_linearity: RadRecLinearity::LinearLti,
            independent_variation: IndependentVariation::TimeVarying,
        }
    }

    /// Class for a direct current-recursion state lane.
    ///
    /// A bare state read is linear in itself with unit, constant coefficient.
    fn current_rec() -> Self {
        Self {
            depends_on_current_rec: true,
            rec_linearity: RadRecLinearity::LinearLti,
            independent_variation: IndependentVariation::Constant,
        }
    }

    /// Class for an expression proven nonlinear in the current recursive
    /// state.
    fn nonlinear() -> Self {
        Self {
            depends_on_current_rec: true,
            rec_linearity: RadRecLinearity::Nonlinear,
            independent_variation: IndependentVariation::Constant,
        }
    }

    /// Combines two expressions under addition or subtraction.
    ///
    /// Addition preserves affine dependence on the current recursive state. If
    /// both operands are independent, only their coefficient variation matters.
    /// If exactly one operand is state-dependent, the independent operand is a
    /// driving term and does not affect the transition class. If both operands
    /// are state-dependent, the result takes the worst state-path linearity.
    fn additive(self, other: Self) -> Self {
        match (self.depends_on_current_rec, other.depends_on_current_rec) {
            (false, false) => Self {
                depends_on_current_rec: false,
                rec_linearity: RadRecLinearity::LinearLti,
                independent_variation: self.independent_variation.max(other.independent_variation),
            },
            (true, false) => self,
            (false, true) => other,
            (true, true) => Self {
                depends_on_current_rec: true,
                rec_linearity: self.rec_linearity.max(other.rec_linearity),
                independent_variation: IndependentVariation::Constant,
            },
        }
    }

    /// Combines two expressions under multiplication.
    ///
    /// Multiplying two current-state-dependent expressions is nonlinear.
    /// Multiplying a state-dependent expression by a state-independent
    /// expression preserves linearity only when the latter is a coefficient; a
    /// time-varying coefficient upgrades the result to
    /// [`RadRecLinearity::LinearTimeVarying`].
    fn multiplicative(self, other: Self) -> Self {
        match (self.depends_on_current_rec, other.depends_on_current_rec) {
            (true, true) => Self::nonlinear(),
            (true, false) => Self {
                depends_on_current_rec: true,
                rec_linearity: self.rec_linearity.with_coeff(other.independent_variation),
                independent_variation: IndependentVariation::Constant,
            },
            (false, true) => Self {
                depends_on_current_rec: true,
                rec_linearity: other.rec_linearity.with_coeff(self.independent_variation),
                independent_variation: IndependentVariation::Constant,
            },
            (false, false) => Self {
                depends_on_current_rec: false,
                rec_linearity: RadRecLinearity::LinearLti,
                independent_variation: self.independent_variation.max(other.independent_variation),
            },
        }
    }

    /// Combines numerator and denominator classes under division.
    ///
    /// Division by an expression that depends on the current recursive state is
    /// nonlinear (`state / state_independent` is affine, but
    /// `state_independent / state` and `state / state` are not). Division by a
    /// time-varying independent denominator is treated as a time-varying
    /// coefficient.
    fn denominator(self, denom: Self) -> Self {
        if denom.depends_on_current_rec {
            return Self::nonlinear();
        }
        if self.depends_on_current_rec {
            Self {
                depends_on_current_rec: true,
                rec_linearity: self.rec_linearity.with_coeff(denom.independent_variation),
                independent_variation: IndependentVariation::Constant,
            }
        } else {
            Self {
                depends_on_current_rec: false,
                rec_linearity: RadRecLinearity::LinearLti,
                independent_variation: self.independent_variation.max(denom.independent_variation),
            }
        }
    }

    /// Applies a smooth or otherwise nonlinear unary numeric primitive.
    ///
    /// These primitives preserve state-independent variation, but any current
    /// recursive-state dependency through the operand becomes nonlinear.
    fn pure_unary(self) -> Self {
        if self.depends_on_current_rec {
            Self::nonlinear()
        } else {
            self
        }
    }

    /// Applies a discrete unary primitive such as integer cast or rounding.
    ///
    /// Discrete operations are not affine in a recursive state lane. On
    /// state-independent values they are still treated as time-varying because
    /// the local rule table does not prove constant folding through them.
    fn discrete_unary(self) -> Self {
        if self.depends_on_current_rec {
            Self::nonlinear()
        } else {
            Self::time_varying()
        }
    }

    /// Applies a delay-like temporal shift.
    ///
    /// A temporal shift of the current recursive state remains structurally
    /// linear for classification purposes. Exact RAD lowering still needs a
    /// block/tape convention because the reverse of a delay is anti-causal in
    /// stream time.
    fn temporal_shift(self) -> Self {
        if self.depends_on_current_rec {
            self
        } else {
            Self {
                depends_on_current_rec: false,
                rec_linearity: RadRecLinearity::LinearLti,
                independent_variation: self.independent_variation,
            }
        }
    }

    /// Combines selector and alternatives under a conditional branch.
    ///
    /// Any current-state dependence in the selector or either branch is
    /// classified as nonlinear. If all operands are state-independent, the
    /// branch is a time-varying driving expression whose variation is the
    /// maximum of its operands.
    fn branch(selector: Self, when_zero: Self, when_nonzero: Self) -> Self {
        if selector.depends_on_current_rec
            || when_zero.depends_on_current_rec
            || when_nonzero.depends_on_current_rec
        {
            Self::nonlinear()
        } else {
            Self {
                depends_on_current_rec: false,
                rec_linearity: RadRecLinearity::LinearLti,
                independent_variation: selector
                    .independent_variation
                    .max(when_zero.independent_variation)
                    .max(when_nonzero.independent_variation),
            }
        }
    }
}

/// Classifies one `DEBRUIJNREC(body)` group for future stateful RAD modes.
///
/// Returns `None` when `group` is not a De Bruijn recursive group. A
/// successful result is purely structural: current `rad(...)` lowering still
/// rejects recursion and delay nodes until a later phase adds a concrete
/// transpose or BPTT implementation.
///
/// # Semantics
/// `body` must be a proper list of recursive branch signals. Each branch is
/// classified with `current_level = 1`, which means `DEBRUIJNREF(1)` denotes
/// the group being classified. The group result is the maximum
/// [`RadRecLinearity`] across all branches.
///
/// A malformed recursive body list returns `None`. That keeps this helper a
/// predicate over well-formed propagated recursion groups rather than a
/// diagnostics-producing validator.
///
/// # Examples
/// In signal notation:
///
/// ```text
/// input + 0.5 * Proj(0, DEBRUIJNREF(1))       => LinearLti
/// input(0) * Proj(0, DEBRUIJNREF(1))          => LinearTimeVarying
/// sin(Proj(0, DEBRUIJNREF(1)))                => Nonlinear
/// ```
#[must_use]
pub fn classify_de_bruijn_rec_group(arena: &TreeArena, group: SigId) -> Option<RadRecLinearity> {
    let body = match_de_bruijn_rec(arena, group)?;
    let branches = list_to_vec(arena, body)?;
    let mut analyzer = LinearityAnalyzer::new();
    Some(
        branches
            .into_iter()
            .map(|branch| analyzer.classify(arena, branch, 1).rec_linearity)
            .fold(RadRecLinearity::LinearLti, RadRecLinearity::max),
    )
}

/// Classifies one `DEBRUIJNREC(body)` group into the RAD strategy it would
/// need in a future stateful reverse-mode implementation.
///
/// Returns `None` for non-`DEBRUIJNREC` inputs. A returned mode is a gate, not
/// an implementation hook: phase-1 `rad(...)` still rejects recursive
/// projections until E1/E2/F add the corresponding runtime semantics.
///
/// This is a convenience wrapper around [`classify_de_bruijn_rec_group`] for
/// call sites that care about the future implementation strategy rather than
/// the raw structural class.
#[must_use]
pub fn classify_de_bruijn_rec_rad_mode(arena: &TreeArena, group: SigId) -> Option<RecRadMode> {
    classify_de_bruijn_rec_group(arena, group).map(RecRadMode::from)
}

/// Classifies a recursive projection such as `Proj(i, DEBRUIJNREC(...))`.
///
/// This is the shape reached by propagated recursive boxes and by RAD's
/// current strict rejection path. Returning `None` means the signal is not a
/// direct projection over a De Bruijn recursive group.
///
/// The projection index itself does not affect the result. A recursive group
/// is classified as a whole because cross-coupled state updates can make one
/// lane's future RAD mode depend on another lane's branch.
#[must_use]
pub fn classify_recursive_projection_rad_mode(arena: &TreeArena, sig: SigId) -> Option<RecRadMode> {
    let SigMatch::Proj(_, group) = match_sig(arena, sig) else {
        return None;
    };
    classify_de_bruijn_rec_rad_mode(arena, group)
}

/// Stateful memoizing classifier for signal expressions under one De Bruijn
/// context.
///
/// The analyzer has no side effects on the arena. It owns only a memo table so
/// the same instance can be reused across all branches of a recursive group,
/// preserving DAG sharing in the analysis itself.
struct LinearityAnalyzer {
    /// Cached abstract classes keyed by signal node and the active De Bruijn
    /// level. The level is part of the key because the same node can be
    /// independent in one nested recursion and current-state-dependent in
    /// another.
    memo: AHashMap<(TreeId, i64), ExprClass>,
}

impl LinearityAnalyzer {
    /// Creates an empty analyzer for one recursive-group classification.
    fn new() -> Self {
        Self {
            memo: AHashMap::new(),
        }
    }

    /// Classifies `sig` under the given current De Bruijn recursion level.
    ///
    /// This memoized wrapper is the only recursive entry point used by the
    /// rule table. Use [`LinearityAnalyzer::classify_uncached`] only to define
    /// the per-signal transfer rules.
    fn classify(&mut self, arena: &TreeArena, sig: SigId, current_level: i64) -> ExprClass {
        if let Some(cached) = self.memo.get(&(sig, current_level)).copied() {
            return cached;
        }

        let class = self.classify_uncached(arena, sig, current_level);
        self.memo.insert((sig, current_level), class);
        class
    }

    /// Applies the structural rule table for one uncached signal node.
    ///
    /// The table is conservative by construction:
    ///
    /// - literal constants are [`ExprClass::constant`];
    /// - inputs, controls, external state, soundfiles, unknown nodes, and
    ///   malformed nested lists are [`ExprClass::time_varying`];
    /// - transparent wrappers forward the class of their signal operand;
    /// - arithmetic `+`/`-` use [`ExprClass::additive`];
    /// - arithmetic `*` and `/` preserve affine state dependence only when the
    ///   other operand is state-independent;
    /// - smooth nonlinear primitives, branches, tables, foreign functions, and
    ///   discrete operations become [`ExprClass::nonlinear`] when they consume
    ///   the current recursive state.
    ///
    /// Nested `DEBRUIJNREC` bodies are classified at `current_level + 1`, so
    /// references to the outer group are no longer confused with references to
    /// the nested group.
    fn classify_uncached(
        &mut self,
        arena: &TreeArena,
        sig: SigId,
        current_level: i64,
    ) -> ExprClass {
        if match_de_bruijn_ref(arena, sig) == Some(current_level) {
            return ExprClass::current_rec();
        }
        if match_de_bruijn_ref(arena, sig).is_some() {
            return ExprClass::time_varying();
        }
        if let Some(body) = match_de_bruijn_rec(arena, sig) {
            let Some(branches) = list_to_vec(arena, body) else {
                return ExprClass::time_varying();
            };
            return branches
                .into_iter()
                .map(|branch| self.classify(arena, branch, current_level + 1))
                .fold(ExprClass::time_varying(), ExprClass::additive);
        }

        match match_sig(arena, sig) {
            SigMatch::Int(_) | SigMatch::Real(_) => ExprClass::constant(),
            SigMatch::Input(_)
            | SigMatch::Button(_)
            | SigMatch::Checkbox(_)
            | SigMatch::VSlider(_)
            | SigMatch::HSlider(_)
            | SigMatch::NumEntry(_)
            | SigMatch::FConst(_, _, _)
            | SigMatch::FVar(_, _, _)
            | SigMatch::Soundfile(_) => ExprClass::time_varying(),
            SigMatch::Output(_, x)
            | SigMatch::FloatCast(x)
            | SigMatch::VBargraph(_, x)
            | SigMatch::HBargraph(_, x)
            | SigMatch::Lowest(x)
            | SigMatch::Highest(x)
            | SigMatch::Clocked(_, x) => self.classify(arena, x, current_level),
            SigMatch::IntCast(x)
            | SigMatch::BitCast(x)
            | SigMatch::Gen(x)
            | SigMatch::Floor(x)
            | SigMatch::Ceil(x)
            | SigMatch::Rint(x)
            | SigMatch::Round(x) => self.classify(arena, x, current_level).discrete_unary(),
            SigMatch::Delay1(x) => self.classify(arena, x, current_level).temporal_shift(),
            SigMatch::Delay(x, amount) => {
                let x_class = self.classify(arena, x, current_level);
                let amount_class = self.classify(arena, amount, current_level);
                if amount_class.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else if amount_class.independent_variation == IndependentVariation::TimeVarying {
                    ExprClass {
                        depends_on_current_rec: x_class.depends_on_current_rec,
                        rec_linearity: x_class
                            .rec_linearity
                            .max(RadRecLinearity::LinearTimeVarying),
                        independent_variation: x_class.independent_variation,
                    }
                } else {
                    x_class.temporal_shift()
                }
            }
            SigMatch::Fir(coefs) => self.classify_fir_carrier(arena, coefs, current_level),
            SigMatch::Iir(coefs) => self.classify_iir_carrier(arena, coefs, current_level),
            SigMatch::Prefix(init, x) => {
                let init_class = self.classify(arena, init, current_level);
                let x_class = self.classify(arena, x, current_level);
                if init_class.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    x_class.temporal_shift()
                }
            }
            SigMatch::BinOp(op, x, y) => {
                let lhs = self.classify(arena, x, current_level);
                let rhs = self.classify(arena, y, current_level);
                match op {
                    BinOp::Add | BinOp::Sub => lhs.additive(rhs),
                    BinOp::Mul => lhs.multiplicative(rhs),
                    BinOp::Div => lhs.denominator(rhs),
                    BinOp::Rem
                    | BinOp::Lsh
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
                    | BinOp::Xor => {
                        if lhs.depends_on_current_rec || rhs.depends_on_current_rec {
                            ExprClass::nonlinear()
                        } else {
                            ExprClass::time_varying()
                        }
                    }
                }
            }
            SigMatch::Pow(x, y)
            | SigMatch::Min(x, y)
            | SigMatch::Max(x, y)
            | SigMatch::Atan2(x, y)
            | SigMatch::Fmod(x, y)
            | SigMatch::Remainder(x, y)
            | SigMatch::ZeroPad(x, y)
            | SigMatch::Seq(x, y) => {
                let lhs = self.classify(arena, x, current_level);
                let rhs = self.classify(arena, y, current_level);
                if lhs.depends_on_current_rec || rhs.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::Sin(x)
            | SigMatch::Cos(x)
            | SigMatch::Tan(x)
            | SigMatch::Exp(x)
            | SigMatch::Exp10(x)
            | SigMatch::Log(x)
            | SigMatch::Log10(x)
            | SigMatch::Sqrt(x)
            | SigMatch::Abs(x)
            | SigMatch::Acos(x)
            | SigMatch::Asin(x)
            | SigMatch::Atan(x) => self.classify(arena, x, current_level).pure_unary(),
            SigMatch::Select2(selector, when_zero, when_nonzero) => ExprClass::branch(
                self.classify(arena, selector, current_level),
                self.classify(arena, when_zero, current_level),
                self.classify(arena, when_nonzero, current_level),
            ),
            SigMatch::AssertBounds(x, lo, hi) => {
                let x_class = self.classify(arena, x, current_level);
                let lo_class = self.classify(arena, lo, current_level);
                let hi_class = self.classify(arena, hi, current_level);
                if lo_class.depends_on_current_rec || hi_class.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    x_class
                }
            }
            SigMatch::RdTbl(table, ridx) => {
                let table_class = self.classify(arena, table, current_level);
                let idx_class = self.classify(arena, ridx, current_level);
                if table_class.depends_on_current_rec || idx_class.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::WrTbl(size, generator, wi, ws)
            | SigMatch::SoundfileBuffer(size, generator, wi, ws) => {
                let combined = self
                    .classify(arena, size, current_level)
                    .additive(self.classify(arena, generator, current_level))
                    .additive(self.classify(arena, wi, current_level))
                    .additive(self.classify(arena, ws, current_level));
                if combined.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::SoundfileLength(sf, part) | SigMatch::SoundfileRate(sf, part) => {
                let combined = self
                    .classify(arena, sf, current_level)
                    .additive(self.classify(arena, part, current_level));
                if combined.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::FFun(_, largs) => {
                let Some(args) = list_to_vec(arena, largs) else {
                    return ExprClass::time_varying();
                };
                let combined = args
                    .into_iter()
                    .map(|arg| self.classify(arena, arg, current_level))
                    .fold(ExprClass::constant(), ExprClass::additive);
                if combined.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::Proj(_, group) => match match_de_bruijn_ref(arena, group) {
                Some(level) if level == current_level => ExprClass::current_rec(),
                Some(_) => ExprClass::time_varying(),
                None => {
                    let group_class = self.classify(arena, group, current_level);
                    if group_class.depends_on_current_rec {
                        group_class
                    } else {
                        ExprClass::time_varying()
                    }
                }
            },
            SigMatch::Rec(body) | SigMatch::ReverseTimeRec(body) => {
                self.classify(arena, body, current_level)
            }
            SigMatch::Attach(x, y) | SigMatch::Enable(x, y) | SigMatch::Control(x, y) => {
                let x_class = self.classify(arena, x, current_level);
                let y_class = self.classify(arena, y, current_level);
                if y_class.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    x_class
                }
            }
            SigMatch::Waveform(values)
            | SigMatch::OnDemand(values)
            | SigMatch::Upsampling(values)
            | SigMatch::Downsampling(values) => {
                let combined = values
                    .iter()
                    .copied()
                    .map(|x| self.classify(arena, x, current_level))
                    .fold(ExprClass::constant(), ExprClass::additive);
                if combined.depends_on_current_rec {
                    ExprClass::nonlinear()
                } else {
                    ExprClass::time_varying()
                }
            }
            SigMatch::TempVar(x) | SigMatch::PermVar(x) => self.classify(arena, x, current_level),
            // The block-reverse-AD carrier is opaque to the LTI/affine
            // classifier: by construction it is only produced as the
            // general fallback when the symbolic and LTI paths refuse, so
            // its body is not eligible for state-space transposition.
            // Treat it as a nonlinear leaf — the dispatcher should never
            // route it back into the LTI bridge.
            SigMatch::BlockReverseAD { .. } => ExprClass::nonlinear(),
            SigMatch::Unknown => ExprClass::time_varying(),
            // Opaque clock-env token: never a signal operand; classify
            // conservatively if it is ever reached through a malformed tree.
            SigMatch::ClockEnvToken(_) => ExprClass::time_varying(),
        }
    }

    /// Classifies `sigFIR([base, tap0, tap1, ...])` as the sum of delayed base
    /// terms multiplied by independent coefficients.
    fn classify_fir_carrier(
        &mut self,
        arena: &TreeArena,
        coefs: &[SigId],
        current_level: i64,
    ) -> ExprClass {
        if coefs.len() < 2 {
            return ExprClass::time_varying();
        }

        let base = self.classify(arena, coefs[0], current_level);
        coefs
            .iter()
            .copied()
            .enumerate()
            .skip(1)
            .map(|(idx, coef)| {
                let source = if idx == 1 {
                    base
                } else {
                    base.temporal_shift()
                };
                source.multiplicative(self.classify(arena, coef, current_level))
            })
            .fold(ExprClass::constant(), ExprClass::additive)
    }

    /// Classifies `sigIIR([rt, input, fb0, fb1, ...])` as an input term plus
    /// delayed recursive-target terms scaled by feedback coefficients.
    fn classify_iir_carrier(
        &mut self,
        arena: &TreeArena,
        coefs: &[SigId],
        current_level: i64,
    ) -> ExprClass {
        if coefs.len() < 2 {
            return ExprClass::time_varying();
        }

        let target = self
            .classify(arena, coefs[0], current_level)
            .temporal_shift();
        let input = self.classify(arena, coefs[1], current_level);
        coefs
            .iter()
            .copied()
            .skip(2)
            .map(|coef| target.multiplicative(self.classify(arena, coef, current_level)))
            .fold(input, ExprClass::additive)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RadRecLinearity, RecRadMode, classify_de_bruijn_rec_group, classify_de_bruijn_rec_rad_mode,
        classify_recursive_projection_rad_mode,
    };
    use signals::{SigBuilder, SigId};
    use tlib::{TreeArena, de_bruijn_rec, de_bruijn_ref, vec_to_list};

    fn one_branch_rec(arena: &mut TreeArena, branch: SigId) -> SigId {
        let body = vec_to_list(arena, &[branch]);
        de_bruijn_rec(arena, body)
    }

    #[test]
    fn classifier_accepts_constant_coefficient_linear_recursion_as_lti() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let half = b.real(0.5);
            let scaled_prev = b.mul(half, prev);
            let input = b.input(0);
            b.add(input, scaled_prev)
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearLti)
        );
    }

    #[test]
    fn classifier_marks_signal_dependent_coefficient_as_time_varying() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let coeff = b.input(0);
            b.mul(coeff, prev)
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearTimeVarying)
        );
    }

    #[test]
    fn classifier_marks_transcendental_of_recursive_state_as_nonlinear() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            b.sin(prev)
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::Nonlinear)
        );
    }

    #[test]
    fn classifier_treats_delay1_of_recursive_state_as_lti_shift() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            b.delay1(prev)
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearLti)
        );
    }

    #[test]
    fn classifier_accepts_multi_output_cross_coupled_lti_recursion() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let (branch0, branch1) = {
            let mut b = SigBuilder::new(&mut arena);
            let prev0 = b.proj(0, ref1);
            let prev1 = b.proj(1, ref1);
            let half = b.real(0.5);
            let scaled_prev1 = b.mul(half, prev1);
            let input = b.input(0);
            let branch0 = b.add(input, scaled_prev1);
            let branch1 = b.sub(prev0, input);
            (branch0, branch1)
        };
        let body = vec_to_list(&mut arena, &[branch0, branch1]);
        let rec = de_bruijn_rec(&mut arena, body);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearLti)
        );
    }

    #[test]
    fn classifier_treats_fir_carrier_over_recursive_state_as_lti() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let c0 = b.real(0.5);
            let c1 = b.real(-0.25);
            b.fir(&[prev, c0, c1])
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearLti)
        );
    }

    #[test]
    fn classifier_treats_iir_carrier_with_signal_feedback_as_ltv() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let input = b.input(0);
            let fb = b.input(1);
            b.iir(&[prev, input, fb])
        };
        let rec = one_branch_rec(&mut arena, branch);

        assert_eq!(
            classify_de_bruijn_rec_group(&arena, rec),
            Some(RadRecLinearity::LinearTimeVarying)
        );
    }

    #[test]
    fn rec_rad_mode_maps_linearity_classes_to_phase_targets() {
        assert_eq!(
            RecRadMode::from(RadRecLinearity::LinearLti),
            RecRadMode::LinearTranspose
        );
        assert_eq!(
            RecRadMode::from(RadRecLinearity::LinearTimeVarying),
            RecRadMode::BlockLinearTimeVarying
        );
        assert_eq!(
            RecRadMode::from(RadRecLinearity::Nonlinear),
            RecRadMode::BpttRequired
        );
    }

    #[test]
    fn classifier_reports_rad_mode_for_de_bruijn_group_and_projection() {
        let mut arena = TreeArena::new();
        let ref1 = de_bruijn_ref(&mut arena, 1);
        let branch = {
            let mut b = SigBuilder::new(&mut arena);
            let prev = b.proj(0, ref1);
            let coeff = b.input(0);
            b.mul(coeff, prev)
        };
        let rec = one_branch_rec(&mut arena, branch);
        let proj = SigBuilder::new(&mut arena).proj(0, rec);

        assert_eq!(
            classify_de_bruijn_rec_rad_mode(&arena, rec),
            Some(RecRadMode::BlockLinearTimeVarying)
        );
        assert_eq!(
            classify_recursive_projection_rad_mode(&arena, proj),
            Some(RecRadMode::BlockLinearTimeVarying)
        );
    }
}
