//! Signal construction and matching helpers backed by `tlib::TreeArena`.
//!
//! # Source provenance (C++)
//! - `compiler/signals/signals.hh`
//! - `compiler/signals/signals.cpp`
//! - `compiler/signals/binop.hh`
//!
//! # Public API mapping status
//! - Public construction API is [`SigBuilder`], aligned with the canonical
//!   `BoxBuilder` style used in `crates/boxes`.
//! - Public inspection API is [`match_sig`] + [`SigMatch`].
//!
//! # Parity invariants
//! - Signal nodes are represented as tagged trees with deterministic child order.
//! - Numeric constants are direct `Int` / `FloatBits` nodes.
//! - UI control signals carry stable [`ui::ControlId`] references only; grouped
//!   labels/ranges/layout live in `crates/ui`.
//! - `sigDoubleClocked(inside, outside, y)` keeps the C++ nested representation
//!   `sigClocked(inside, sigClocked(outside, y))` instead of introducing a
//!   separate Rust-only node family.
//! - `ReverseTimeRec(group)` is a Rust-only phase-E1 RAD carrier. It wraps a
//!   normal recursive group and keeps the usual body/projection contract, but
//!   downstream lowering must evaluate the group from the end of the current
//!   compute block back to the beginning with terminal adjoint state initialized
//!   to zero. RAD propagation does not currently produce this node; it remains
//!   dormant IR-level carrier infrastructure for a future LTI fast-path revival
//!   documented in
//!   `porting/rad-disable-reverse-time-rec-fast-path-plan-2026-05-10-en.md`.
//! - `BlockReverseAD { body, primal_count, seeds, cotangents, policy }` is a
//!   Rust-only general RAD fallback carrier. It is the Signal-IR-level
//!   counterpart of the block-local Truncated BPTT model described in
//!   `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`. The
//!   carrier is opaque to the symbolic feed-forward and LTI fast paths; its
//!   outputs are addressed via `Proj(slot, group)` with primal outputs first
//!   and per-sample seed gradients after.
//! - `Fir` and `Iir` are C++-parity filter carrier nodes for the structured
//!   LTI algebra port documented in
//!   `porting/lti-filter-intermediate-form-plan-2026-05-06-en.md`. They mirror
//!   C++ `sigFIR` / `sigIIR` storage exactly as vector-valued signal nodes; the
//!   algebraic helpers that reveal and transform those nodes live above this
//!   representation layer.
//!
//! # Integer convention
//! - Public signal integer surface (`SigBuilder::int`, `SigMatch::Int`, and
//!   index-bearing shapes such as `Input/Output/Proj`) uses `i32` semantics.
//! - Underlying arena storage remains `NodeKind::Int(i64)`; this crate owns the
//!   narrowing conversion at decode boundaries.

use std::fmt::Write;

use tlib::{NodeKind, TreeArena, TreeId};
use ui::ControlId;

mod filter_algebra;

pub mod ad_rules;

pub use filter_algebra::{
    FilterStateSpaceError, FirFilter, IirFilter, LinearTerm, StateSpace, add_sig_fir, add_sig_iir,
    concerned_iir, convert_fir_to_sig, delay_sig_fir, delay_sig_iir, div_sig_iir, embedded_iir,
    extract_fir_filter, extract_iir_filter, iir_filter_to_state_space, make_sig_fir, mul_sig_iir,
    neg_sig_fir, proj_to_sig_iir, simplify_fir, sub_sig_fir, sub_sig_iir,
};

pub const CRATE_NAME: &str = "signals";

/// Signal node identifier in `TreeArena`.
pub type SigId = TreeId;

const SIG_INPUT_TAG: &str = "SIGINPUT";
const SIG_OUTPUT_TAG: &str = "SIGOUTPUT";
const SIG_DELAY1_TAG: &str = "SIGDELAY1";
const SIG_DELAY_TAG: &str = "SIGDELAY";
const SIG_PREFIX_TAG: &str = "SIGPREFIX";
const SIG_INT_CAST_TAG: &str = "SIGINTCAST";
const SIG_BIT_CAST_TAG: &str = "SIGBITCAST";
const SIG_FLOAT_CAST_TAG: &str = "SIGFLOATCAST";
const SIG_RDTBL_TAG: &str = "SIGRDTBL";
const SIG_WRTBL_TAG: &str = "SIGWRTBL";
const SIG_GEN_TAG: &str = "SIGGEN";
const SIG_SELECT2_TAG: &str = "SIGSELECT2";
const SIG_ASSERT_BOUNDS_TAG: &str = "SIGASSERTBOUNDS";
const SIG_LOWEST_TAG: &str = "SIGLOWEST";
const SIG_HIGHEST_TAG: &str = "SIGHIGHEST";
const SIG_BINOP_TAG: &str = "SIGBINOP";
const SIG_POW_TAG: &str = "SIGPOW";
const SIG_MIN_TAG: &str = "SIGMIN";
const SIG_MAX_TAG: &str = "SIGMAX";
const SIG_ACOS_TAG: &str = "SIGACOS";
const SIG_ASIN_TAG: &str = "SIGASIN";
const SIG_ATAN_TAG: &str = "SIGATAN";
const SIG_ATAN2_TAG: &str = "SIGATAN2";
const SIG_COS_TAG: &str = "SIGCOS";
const SIG_SIN_TAG: &str = "SIGSIN";
const SIG_TAN_TAG: &str = "SIGTAN";
const SIG_EXP_TAG: &str = "SIGEXP";
const SIG_EXP10_TAG: &str = "SIGEXP10";
const SIG_LOG_TAG: &str = "SIGLOG";
const SIG_LOG10_TAG: &str = "SIGLOG10";
const SIG_SQRT_TAG: &str = "SIGSQRT";
const SIG_ABS_TAG: &str = "SIGABS";
const SIG_FMOD_TAG: &str = "SIGFMOD";
const SIG_REMAINDER_TAG: &str = "SIGREMAINDER";
const SIG_FLOOR_TAG: &str = "SIGFLOOR";
const SIG_CEIL_TAG: &str = "SIGCEIL";
const SIG_RINT_TAG: &str = "SIGRINT";
const SIG_ROUND_TAG: &str = "SIGROUND";
const SIG_FFUN_TAG: &str = "SIGFFUN";
const SIG_FCONST_TAG: &str = "SIGFCONST";
const SIG_FVAR_TAG: &str = "SIGFVAR";
const SIG_PROJ_TAG: &str = "SIGPROJ";
const SIG_BUTTON_TAG: &str = "SIGBUTTON";
const SIG_CHECKBOX_TAG: &str = "SIGCHECKBOX";
const SIG_VSLIDER_TAG: &str = "SIGVSLIDER";
const SIG_HSLIDER_TAG: &str = "SIGHSLIDER";
const SIG_NUMENTRY_TAG: &str = "SIGNUMENTRY";
const SIG_VBARGRAPH_TAG: &str = "SIGVBARGRAPH";
const SIG_HBARGRAPH_TAG: &str = "SIGHBARGRAPH";
const SIG_ATTACH_TAG: &str = "SIGATTACH";
const SIG_ENABLE_TAG: &str = "SIGENABLE";
const SIG_CONTROL_TAG: &str = "SIGCONTROL";
const SIG_WAVEFORM_TAG: &str = "SIGWAVEFORM";
const SIG_SOUNDFILE_TAG: &str = "SIGSOUNDFILE";
const SIG_SOUNDFILE_LENGTH_TAG: &str = "SIGSOUNDFILELENGTH";
const SIG_SOUNDFILE_RATE_TAG: &str = "SIGSOUNDFILERATE";
const SIG_SOUNDFILE_BUFFER_TAG: &str = "SIGSOUNDFILEBUFFER";
const SIG_TEMPVAR_TAG: &str = "SIGTEMPVAR";
const SIG_PERMVAR_TAG: &str = "SIGPERMVAR";
const SIG_SEQ_TAG: &str = "SIGSEQ";
const SIG_ZEROPAD_TAG: &str = "SIGZEROPAD";
const SIG_OD_TAG: &str = "SIGOD";
const SIG_US_TAG: &str = "SIGUS";
const SIG_DS_TAG: &str = "SIGDS";
const SIG_CLOCKED_TAG: &str = "SIGCLOCKED";
const SIG_CLOCKENV_TAG: &str = "SIGCLOCKENV";
const SIG_REC_TAG: &str = "SIGREC";
const SIG_REVERSE_TIME_REC_TAG: &str = "SIGREVERSETIMEREC";
const SIG_FIR_TAG: &str = "SIGFIR";
const SIG_IIR_TAG: &str = "SIGIIR";
const SIG_BLOCK_REVERSE_AD_TAG: &str = "SIGBLOCKREVERSEAD";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// Tape policy hint carried by [`SigMatch::BlockReverseAD`].
///
/// Phase B0 of the block-reverse-AD plan only specifies and exercises
/// [`BlockRevPolicy::TapeFull`]. The other variants are reserved so a future
/// backend implementation can opt into checkpointing or pure recompute
/// without changing the on-arena layout of the carrier.
///
/// Source provenance: original Rust design in
/// `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`,
/// section "8. Tape And Checkpointing Policy".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i64)]
pub enum BlockRevPolicy {
    /// Record every active value referenced by a reverse rule for the
    /// current block. Memory cost is `block_size × active_value_count` of
    /// the body's real precision. The only variant Phase B0 commits to.
    TapeFull = 0,
    /// Reserved: split the block into checkpointed segments and recompute
    /// missing tape entries on demand. Not implemented in Phase B0.
    Checkpointed = 1,
    /// Reserved: discard the tape entirely and recompute the forward sweep
    /// inside the reverse loop. Not implemented in Phase B0.
    Recompute = 2,
}

impl BlockRevPolicy {
    /// Decodes a raw `i64` (as stored on the arena) back into a policy.
    /// Returns `None` for unknown tags so callers can surface a structured
    /// validation diagnostic rather than panicking.
    #[must_use]
    pub fn from_raw(raw: i64) -> Option<Self> {
        match raw {
            0 => Some(Self::TapeFull),
            1 => Some(Self::Checkpointed),
            2 => Some(Self::Recompute),
            _ => None,
        }
    }

    /// Encodes the policy as the raw `i64` stored under the carrier's
    /// fifth child.
    #[must_use]
    pub fn to_raw(self) -> i64 {
        self as i64
    }
}

/// Binary signal operators (aligned with C++ `SOperator` order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i64)]
pub enum BinOp {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
    Rem = 4,
    Lsh = 5,
    ARsh = 6,
    LRsh = 7,
    Gt = 8,
    Lt = 9,
    Ge = 10,
    Le = 11,
    Eq = 12,
    Ne = 13,
    And = 14,
    Or = 15,
    Xor = 16,
}

impl BinOp {
    #[must_use]
    /// Executes this operation and returns its result.
    pub fn from_raw(raw: i64) -> Option<Self> {
        match raw {
            0 => Some(Self::Add),
            1 => Some(Self::Sub),
            2 => Some(Self::Mul),
            3 => Some(Self::Div),
            4 => Some(Self::Rem),
            5 => Some(Self::Lsh),
            6 => Some(Self::ARsh),
            7 => Some(Self::LRsh),
            8 => Some(Self::Gt),
            9 => Some(Self::Lt),
            10 => Some(Self::Ge),
            11 => Some(Self::Le),
            12 => Some(Self::Eq),
            13 => Some(Self::Ne),
            14 => Some(Self::And),
            15 => Some(Self::Or),
            16 => Some(Self::Xor),
            _ => None,
        }
    }

    #[must_use]
    /// Executes this operation and returns its result.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::Lsh => "<<",
            Self::ARsh => ">>",
            Self::LRsh => ">>>",
            Self::Gt => ">",
            Self::Lt => "<",
            Self::Ge => ">=",
            Self::Le => "<=",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::And => "&",
            Self::Or => "|",
            Self::Xor => "^",
        }
    }

    #[must_use]
    /// Executes this operation and returns its result.
    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Rem => "rem",
            Self::Lsh => "lsh",
            Self::ARsh => "arsh",
            Self::LRsh => "lrsh",
            Self::Gt => "gt",
            Self::Lt => "lt",
            Self::Ge => "ge",
            Self::Le => "le",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
        }
    }
}

/// Canonical builder API for constructing signal nodes.
///
/// Builder methods preserve the canonical surface expected by `eval`,
/// `propagate`, dumps, and fast-lane lowering. They normalize only local
/// encodings such as slider parameter lists and obvious cast no-ops.
pub struct SigBuilder<'a> {
    arena: &'a mut TreeArena,
}

impl<'a> SigBuilder<'a> {
    fn debug_assert_non_negative_index(kind: &str, index: i32) {
        debug_assert!(index >= 0, "{kind} index must be non-negative, got {index}");
    }

    #[must_use]
    /// Creates a `SigBuilder` bound to one mutable `TreeArena`.
    pub fn new(arena: &'a mut TreeArena) -> Self {
        Self { arena }
    }

    #[must_use]
    pub(crate) fn arena(&self) -> &TreeArena {
        self.arena
    }

    #[must_use]
    /// Builds one signal node for `int` and returns its `SigId`.
    pub fn int(&mut self, n: i32) -> SigId {
        self.arena.int(i64::from(n))
    }

    #[must_use]
    /// Builds one signal node for `real` and returns its `SigId`.
    pub fn real(&mut self, r: f64) -> SigId {
        self.arena.float(r)
    }

    #[must_use]
    /// Builds one signal node for `input` and returns its `SigId`.
    pub fn input(&mut self, index: i32) -> SigId {
        Self::debug_assert_non_negative_index("SIGINPUT", index);
        let idx = self.arena.int(i64::from(index));
        intern_tag(self.arena, SIG_INPUT_TAG, &[idx])
    }

    #[must_use]
    /// Builds one signal node for `output` and returns its `SigId`.
    pub fn output(&mut self, index: i32, sig: SigId) -> SigId {
        Self::debug_assert_non_negative_index("SIGOUTPUT", index);
        let idx = self.arena.int(i64::from(index));
        intern_tag(self.arena, SIG_OUTPUT_TAG, &[idx, sig])
    }

    #[must_use]
    /// Builds one signal node for `delay1` and returns its `SigId`.
    pub fn delay1(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_DELAY1_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `delay` and returns its `SigId`.
    pub fn delay(&mut self, sig: SigId, amount: SigId) -> SigId {
        intern_tag(self.arena, SIG_DELAY_TAG, &[sig, amount])
    }

    #[must_use]
    /// Builds one signal node for `prefix` and returns its `SigId`.
    pub fn prefix(&mut self, init: SigId, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_PREFIX_TAG, &[init, sig])
    }

    #[must_use]
    /// Builds one signal node for `int_cast` and returns its `SigId`.
    pub fn int_cast(&mut self, sig: SigId) -> SigId {
        if matches!(self.arena.kind(sig), Some(NodeKind::Int(_))) {
            sig
        } else {
            intern_tag(self.arena, SIG_INT_CAST_TAG, &[sig])
        }
    }

    #[must_use]
    /// Builds one signal node for `bit_cast` and returns its `SigId`.
    pub fn bit_cast(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_BIT_CAST_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `float_cast` and returns its `SigId`.
    pub fn float_cast(&mut self, sig: SigId) -> SigId {
        match self.arena.kind(sig) {
            Some(NodeKind::Int(v)) => self.arena.float(*v as f64),
            Some(NodeKind::FloatBits(_)) => sig,
            _ => intern_tag(self.arena, SIG_FLOAT_CAST_TAG, &[sig]),
        }
    }

    #[must_use]
    /// Builds one signal node for `generate` and returns its `SigId`.
    pub fn generate(&mut self, content: SigId) -> SigId {
        intern_tag(self.arena, SIG_GEN_TAG, &[content])
    }

    #[must_use]
    /// Builds one signal node for `wrtbl` and returns its `SigId`.
    pub fn wrtbl(&mut self, size: SigId, generator: SigId, widx: SigId, wsig: SigId) -> SigId {
        intern_tag(self.arena, SIG_WRTBL_TAG, &[size, generator, widx, wsig])
    }

    #[must_use]
    /// Builds one signal node for `wrtbl_readonly` and returns its `SigId`.
    pub fn wrtbl_readonly(&mut self, size: SigId, generator: SigId) -> SigId {
        let nil = self.arena.nil();
        self.wrtbl(size, generator, nil, nil)
    }

    #[must_use]
    /// Builds one signal node for `rdtbl` and returns its `SigId`.
    pub fn rdtbl(&mut self, tbl: SigId, ridx: SigId) -> SigId {
        intern_tag(self.arena, SIG_RDTBL_TAG, &[tbl, ridx])
    }

    #[must_use]
    /// Builds one signal node for `write_read_table` and returns its `SigId`.
    pub fn write_read_table(
        &mut self,
        size: SigId,
        init: SigId,
        widx: SigId,
        wsig: SigId,
        ridx: SigId,
    ) -> SigId {
        let generator = self.generate(init);
        let tbl = self.wrtbl(size, generator, widx, wsig);
        self.rdtbl(tbl, ridx)
    }

    #[must_use]
    /// Builds one signal node for `read_only_table` and returns its `SigId`.
    pub fn read_only_table(&mut self, size: SigId, init: SigId, ridx: SigId) -> SigId {
        let generator = self.generate(init);
        let tbl = self.wrtbl_readonly(size, generator);
        self.rdtbl(tbl, ridx)
    }

    #[must_use]
    /// Builds one signal node for `select2` and returns its `SigId`.
    pub fn select2(&mut self, selector: SigId, s1: SigId, s2: SigId) -> SigId {
        intern_tag(self.arena, SIG_SELECT2_TAG, &[selector, s1, s2])
    }

    #[must_use]
    /// Builds one signal node for `select3` and returns its `SigId`.
    pub fn select3(&mut self, selector: SigId, s1: SigId, s2: SigId, s3: SigId) -> SigId {
        let zero = self.int(0);
        let one = self.int(1);
        let eq0 = self.eq(selector, zero);
        let eq1 = self.eq(selector, one);
        let inner = self.select2(eq1, s3, s2);
        self.select2(eq0, inner, s1)
    }

    #[must_use]
    /// Builds one signal node for `assert_bounds` and returns its `SigId`.
    pub fn assert_bounds(&mut self, s1: SigId, s2: SigId, s3: SigId) -> SigId {
        intern_tag(self.arena, SIG_ASSERT_BOUNDS_TAG, &[s1, s2, s3])
    }

    #[must_use]
    /// Builds one signal node for `lowest` and returns its `SigId`.
    pub fn lowest(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_LOWEST_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `highest` and returns its `SigId`.
    pub fn highest(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_HIGHEST_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `binop` and returns its `SigId`.
    pub fn binop(&mut self, op: BinOp, x: SigId, y: SigId) -> SigId {
        let opn = self.arena.int(op as i64);
        intern_tag(self.arena, SIG_BINOP_TAG, &[opn, x, y])
    }

    #[must_use]
    /// Builds one signal node for `add` and returns its `SigId`.
    pub fn add(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Add, x, y)
    }

    #[must_use]
    /// Builds one signal node for `sub` and returns its `SigId`.
    pub fn sub(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Sub, x, y)
    }

    #[must_use]
    /// Builds one signal node for `mul` and returns its `SigId`.
    pub fn mul(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Mul, x, y)
    }

    #[must_use]
    /// Builds one signal node for `div` and returns its `SigId`.
    pub fn div(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Div, x, y)
    }

    #[must_use]
    /// Builds one signal node for `rem` and returns its `SigId`.
    pub fn rem(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Rem, x, y)
    }

    #[must_use]
    /// Builds one signal node for `and` and returns its `SigId`.
    pub fn and(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::And, x, y)
    }

    #[must_use]
    /// Builds one signal node for `or` and returns its `SigId`.
    pub fn or(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Or, x, y)
    }

    #[must_use]
    /// Builds one signal node for `xor` and returns its `SigId`.
    pub fn xor(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Xor, x, y)
    }

    #[must_use]
    /// Builds one signal node for `lsh` and returns its `SigId`.
    pub fn lsh(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Lsh, x, y)
    }

    #[must_use]
    /// Builds one signal node for `arsh` and returns its `SigId`.
    pub fn arsh(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::ARsh, x, y)
    }

    #[must_use]
    /// Builds one signal node for `lrsh` and returns its `SigId`.
    pub fn lrsh(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::LRsh, x, y)
    }

    #[must_use]
    /// Builds one signal node for `gt` and returns its `SigId`.
    pub fn gt(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Gt, x, y)
    }

    #[must_use]
    /// Builds one signal node for `lt` and returns its `SigId`.
    pub fn lt(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Lt, x, y)
    }

    #[must_use]
    /// Builds one signal node for `ge` and returns its `SigId`.
    pub fn ge(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Ge, x, y)
    }

    #[must_use]
    /// Builds one signal node for `le` and returns its `SigId`.
    pub fn le(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Le, x, y)
    }

    #[must_use]
    /// Builds one signal node for `eq` and returns its `SigId`.
    pub fn eq(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Eq, x, y)
    }

    #[must_use]
    /// Builds one signal node for `ne` and returns its `SigId`.
    pub fn ne(&mut self, x: SigId, y: SigId) -> SigId {
        self.binop(BinOp::Ne, x, y)
    }

    #[must_use]
    /// Builds one signal node for `pow` and returns its `SigId`.
    pub fn pow(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_POW_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `min` and returns its `SigId`.
    pub fn min(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_MIN_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `max` and returns its `SigId`.
    pub fn max(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_MAX_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `acos` and returns its `SigId`.
    pub fn acos(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_ACOS_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `asin` and returns its `SigId`.
    pub fn asin(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_ASIN_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `atan` and returns its `SigId`.
    pub fn atan(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_ATAN_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `atan2` and returns its `SigId`.
    pub fn atan2(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_ATAN2_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `cos` and returns its `SigId`.
    pub fn cos(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_COS_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `sin` and returns its `SigId`.
    pub fn sin(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_SIN_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `tan` and returns its `SigId`.
    pub fn tan(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_TAN_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `exp` and returns its `SigId`.
    pub fn exp(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_EXP_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `exp10` and returns its `SigId`.
    pub fn exp10(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_EXP10_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `log` and returns its `SigId`.
    pub fn log(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_LOG_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `log10` and returns its `SigId`.
    pub fn log10(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_LOG10_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `sqrt` and returns its `SigId`.
    pub fn sqrt(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_SQRT_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `abs` and returns its `SigId`.
    pub fn abs(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_ABS_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `fmod` and returns its `SigId`.
    pub fn fmod(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_FMOD_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `remainder` and returns its `SigId`.
    pub fn remainder(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_REMAINDER_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `floor` and returns its `SigId`.
    pub fn floor(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_FLOOR_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `ceil` and returns its `SigId`.
    pub fn ceil(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_CEIL_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `rint` and returns its `SigId`.
    pub fn rint(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_RINT_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `round` and returns its `SigId`.
    pub fn round(&mut self, x: SigId) -> SigId {
        intern_tag(self.arena, SIG_ROUND_TAG, &[x])
    }

    #[must_use]
    /// Builds one signal node for `ffun` and returns its `SigId`.
    pub fn ffun(&mut self, ff: SigId, largs: SigId) -> SigId {
        intern_tag(self.arena, SIG_FFUN_TAG, &[ff, largs])
    }

    #[must_use]
    /// Builds one signal node for `fconst` and returns its `SigId`.
    pub fn fconst(&mut self, ty: SigId, name: SigId, file: SigId) -> SigId {
        intern_tag(self.arena, SIG_FCONST_TAG, &[ty, name, file])
    }

    #[must_use]
    /// Builds one signal node for `fvar` and returns its `SigId`.
    pub fn fvar(&mut self, ty: SigId, name: SigId, file: SigId) -> SigId {
        intern_tag(self.arena, SIG_FVAR_TAG, &[ty, name, file])
    }

    #[must_use]
    /// Builds one signal node for `proj` and returns its `SigId`.
    pub fn proj(&mut self, index: i32, group: SigId) -> SigId {
        Self::debug_assert_non_negative_index("SIGPROJ", index);
        let idx = self.arena.int(i64::from(index));
        intern_tag(self.arena, SIG_PROJ_TAG, &[idx, group])
    }

    #[must_use]
    /// Builds one signal node for `rec` and returns its `SigId`.
    pub fn rec(&mut self, body: SigId) -> SigId {
        intern_tag(self.arena, SIG_REC_TAG, &[body])
    }

    #[must_use]
    /// Builds one signal node for `reverse_time_rec` and returns its `SigId`.
    ///
    /// This node is the phase-E1 RAD counterpart of `rec`: it wraps a normal
    /// recursive group with the same arity and `Proj(slot, group)` projection
    /// contract as `rec`, but a backend must evaluate the group in reverse
    /// sample order over the current compute block. The terminal state after
    /// the last frame is implicitly zero and no adjoint state is preserved
    /// across `compute()` calls.
    ///
    /// RAD propagation does not currently produce this node; it remains as
    /// dormant IR-level carrier infrastructure. See
    /// `porting/rad-disable-reverse-time-rec-fast-path-plan-2026-05-10-en.md`.
    ///
    /// Source provenance: original Rust RAD phase-E1 design in
    /// `porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md`, section
    /// "20.3 Signal-IR: a `ReverseTimeRec` node".
    pub fn reverse_time_rec(&mut self, body: SigId) -> SigId {
        intern_tag(self.arena, SIG_REVERSE_TIME_REC_TAG, &[body])
    }

    #[must_use]
    /// Builds one `block_reverse_ad` carrier and returns its `SigId`.
    ///
    /// `BlockReverseAD` is a Rust-only Signal-IR carrier introduced for
    /// the general fallback path of `rad(expr, seeds)`. Its semantic
    /// contract — independent of any specific backend — is:
    ///
    /// ```text
    /// over the current compute() block:
    ///   forward-evaluate `body` sample-by-sample under normal Faust
    ///     execution semantics;
    ///   record (or checkpoint, depending on `policy`) the active values
    ///     required by the reverse rules;
    ///   run a reverse sweep from frame BS-1 down to 0, seeding each
    ///     primal output with `cotangents[i]`, propagating adjoints
    ///     through every primitive in the FAD-supported surface, and
    ///     accumulating per-sample gradient contributions for each seed.
    /// ```
    ///
    /// # Output layout
    ///
    /// The carrier is addressed exclusively through `Proj(slot, group)`,
    /// using the same projection contract as [`Self::rec`] /
    /// [`Self::reverse_time_rec`]:
    ///
    /// - `Proj(0..M-1, group)` — the `M = body.len()` primal outputs;
    /// - `Proj(M..M+N-1, group)` — the per-sample gradient contribution
    ///   for `seeds[k]`, with `N = seeds.len()`.
    ///
    /// `cotangents.len()` must equal `body.len()`. Phase B0 always fills
    /// the cotangent slot with `1.0` constants (one per primal), matching
    /// the existing `rad(expr, seeds)` convention `J = sum(expr_outputs)`.
    /// The slot is materialised on-arena so a future VJP API can populate
    /// it with user-supplied seeds without bumping the tag.
    ///
    /// # Block semantics
    ///
    /// The reverse sweep is *block-local*: terminal adjoint state at
    /// `BS-1` is implicit zero, no adjoint state is preserved across
    /// `compute()` calls, and gradient outputs are per-sample
    /// contributions (not block-summed scalars). This corresponds to
    /// non-overlapping Truncated BPTT(BS, BS) in the sense of Williams &
    /// Peng (1990); see
    /// `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`,
    /// sections 7 and 7.1.
    ///
    /// # Scheduling boundary
    ///
    /// The carrier is intentionally schedule-agnostic.  It does not say
    /// whether the backend must materialize a distinct generated backward
    /// loop.  FIR lowering may emit a split forward/backward schedule when a
    /// gradient projection is a public output, or emit the same local adjoint
    /// sweep inline when that projection is consumed by a forward-time
    /// expression such as a recursive parameter update.
    ///
    /// That inline case is a code-placement consequence of the surrounding
    /// graph, not a proof that an LTI subgraph was recognized and analytically
    /// transposed.  LTI transposition is a separate, more specialized
    /// derivation strategy; `BlockReverseAD` remains the general block-tape
    /// fallback carrier.
    ///
    /// # On-arena layout
    ///
    /// The five children, in order, are:
    ///
    /// | child | role |
    /// |-------|------|
    /// | `body_list` | cons-list of `SigId` primal outputs (M ≥ 1 elements) |
    /// | `primal_count` | `Int(M)`, set from `body.len()` |
    /// | `seed_list` | cons-list of `SigId` seed leaves (N elements, may be empty) |
    /// | `cotangent_list` | cons-list of `SigId` cotangents (M elements) |
    /// | `policy` | `Int(BlockRevPolicy::to_raw())` |
    ///
    /// Source provenance: original Rust design in
    /// `porting/rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`,
    /// sections 4 and 11.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `body` is empty or
    /// `cotangents.len() != body.len()`. Release builds construct a
    /// carrier that will be rejected at the next `signal_prepare` pass
    /// with a structured `SignalPrepareError::Validation` diagnostic, so
    /// arena-level invariants are never violated.
    pub fn block_reverse_ad(
        &mut self,
        body: &[SigId],
        seeds: &[SigId],
        cotangents: &[SigId],
        policy: BlockRevPolicy,
    ) -> SigId {
        debug_assert!(
            !body.is_empty(),
            "BlockReverseAD requires at least one primal output"
        );
        debug_assert_eq!(
            body.len(),
            cotangents.len(),
            "BlockReverseAD body and cotangent lists must have the same length"
        );
        let body_list = tlib::vec_to_list(self.arena, body);
        let seed_list = tlib::vec_to_list(self.arena, seeds);
        let cotangent_list = tlib::vec_to_list(self.arena, cotangents);
        let primal_count = self.arena.int(body.len() as i64);
        let policy_id = self.arena.int(policy.to_raw());
        intern_tag(
            self.arena,
            SIG_BLOCK_REVERSE_AD_TAG,
            &[
                body_list,
                primal_count,
                seed_list,
                cotangent_list,
                policy_id,
            ],
        )
    }

    #[must_use]
    /// Builds one `sigFIR` carrier and returns its `SigId`.
    ///
    /// Source provenance:
    /// - C++ `compiler/signals/signals.cpp::sigFIR`
    /// - C++ `compiler/signals/sigFIR.hh`
    ///
    /// The branch layout is the C++ layout `[S, C0, C1, ...]`, denoting
    /// `C0*S[n] + C1*S[n-1] + ...`. This method intentionally does not enforce
    /// coefficient-count or trailing-zero invariants; those belong to the
    /// algebraic `sigFIR` helper port, which must also preserve C++ degenerate
    /// cases such as zero or plain-gain fallback.
    pub fn fir(&mut self, sigcoefs: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_FIR_TAG, sigcoefs)
    }

    #[must_use]
    /// Builds one `sigIIR` carrier and returns its `SigId`.
    ///
    /// Source provenance:
    /// - C++ `compiler/signals/signals.cpp::sigIIR`
    /// - C++ `compiler/signals/sigIIR.hh`
    ///
    /// The branch layout is the C++ layout `[V, X, C1, C2, ...]`, denoting
    /// `V[n] = X[n] + C1*V[n-1] + C2*V[n-2] + ...`. The first branch may be a
    /// recursive projection or `nil`, matching the C++ helper convention used
    /// by `revealIIR`.
    pub fn iir(&mut self, sigcoefs: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_IIR_TAG, sigcoefs)
    }

    #[must_use]
    /// Builds one signal node for `button` and returns its `SigId`.
    ///
    /// The signal node stores only a stable [`ui::ControlId`]; display label,
    /// metadata, and grouped layout live in the paired `UiProgram`.
    pub fn button(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_BUTTON_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `checkbox` and returns its `SigId`.
    pub fn checkbox(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_CHECKBOX_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `vslider` and returns its `SigId`.
    pub fn vslider(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_VSLIDER_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `hslider` and returns its `SigId`.
    pub fn hslider(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_HSLIDER_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `numentry` and returns its `SigId`.
    pub fn numentry(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_NUMENTRY_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `vbargraph` and returns its `SigId`.
    ///
    /// The bargraph range and metadata are resolved later through the paired
    /// `UiProgram` control registry.
    pub fn vbargraph(&mut self, control: ControlId, sig: SigId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_VBARGRAPH_TAG, &[control, sig])
    }

    #[must_use]
    /// Builds one signal node for `hbargraph` and returns its `SigId`.
    pub fn hbargraph(&mut self, control: ControlId, sig: SigId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_HBARGRAPH_TAG, &[control, sig])
    }

    #[must_use]
    /// Builds one signal node for `waveform` and returns its `SigId`.
    pub fn waveform(&mut self, values: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_WAVEFORM_TAG, values)
    }

    #[must_use]
    /// Builds one signal node for `soundfile` and returns its `SigId`.
    ///
    /// The associated path/url metadata is owned by `UiProgram`, not by this
    /// signal leaf.
    pub fn soundfile(&mut self, control: ControlId) -> SigId {
        let control = self.arena.int(i64::from(control));
        intern_tag(self.arena, SIG_SOUNDFILE_TAG, &[control])
    }

    #[must_use]
    /// Builds one signal node for `soundfile_length` and returns its `SigId`.
    pub fn soundfile_length(&mut self, soundfile: SigId, part: SigId) -> SigId {
        intern_tag(self.arena, SIG_SOUNDFILE_LENGTH_TAG, &[soundfile, part])
    }

    #[must_use]
    /// Builds one signal node for `soundfile_rate` and returns its `SigId`.
    pub fn soundfile_rate(&mut self, soundfile: SigId, part: SigId) -> SigId {
        intern_tag(self.arena, SIG_SOUNDFILE_RATE_TAG, &[soundfile, part])
    }

    #[must_use]
    /// Builds one signal node for `soundfile_buffer` and returns its `SigId`.
    pub fn soundfile_buffer(
        &mut self,
        soundfile: SigId,
        chan: SigId,
        part: SigId,
        ridx: SigId,
    ) -> SigId {
        intern_tag(
            self.arena,
            SIG_SOUNDFILE_BUFFER_TAG,
            &[soundfile, chan, part, ridx],
        )
    }

    #[must_use]
    /// Builds one signal node for `tempvar` and returns its `SigId`.
    pub fn temp_var(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_TEMPVAR_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `permvar` and returns its `SigId`.
    pub fn perm_var(&mut self, sig: SigId) -> SigId {
        intern_tag(self.arena, SIG_PERMVAR_TAG, &[sig])
    }

    #[must_use]
    /// Builds one signal node for `attach` and returns its `SigId`.
    pub fn attach(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_ATTACH_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `enable` and returns its `SigId`.
    pub fn enable(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_ENABLE_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `control` and returns its `SigId`.
    pub fn control(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_CONTROL_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `seq` and returns its `SigId`.
    pub fn seq(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_SEQ_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `zero_pad` and returns its `SigId`.
    pub fn zero_pad(&mut self, x: SigId, y: SigId) -> SigId {
        intern_tag(self.arena, SIG_ZEROPAD_TAG, &[x, y])
    }

    #[must_use]
    /// Builds one signal node for `on_demand` and returns its `SigId`.
    pub fn on_demand(&mut self, sigs: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_OD_TAG, sigs)
    }

    #[must_use]
    /// Builds one signal node for `upsampling` and returns its `SigId`.
    pub fn upsampling(&mut self, sigs: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_US_TAG, sigs)
    }

    #[must_use]
    /// Builds one signal node for `downsampling` and returns its `SigId`.
    pub fn downsampling(&mut self, sigs: &[SigId]) -> SigId {
        intern_tag(self.arena, SIG_DS_TAG, sigs)
    }

    #[must_use]
    /// Builds one signal node for `clocked` and returns its `SigId`.
    ///
    /// Like C++, this keeps `clocked(clock, clocked(clock, y))` canonical by
    /// returning the inner node unchanged when both clocks are structurally
    /// identical.
    pub fn clocked(&mut self, clock: SigId, sig: SigId) -> SigId {
        if let SigMatch::Clocked(existing_clock, _) = match_sig(self.arena, sig)
            && existing_clock == clock
        {
            return sig;
        }
        intern_tag(self.arena, SIG_CLOCKED_TAG, &[clock, sig])
    }

    #[must_use]
    /// Builds one opaque clock-environment token leaf and returns its `SigId`.
    ///
    /// The token identifies one *instance* of a clocked wrapper
    /// (`ondemand` / `upsampling` / `downsampling`) by an integer id into the
    /// propagation-owned `ClockDomain` side table (roadmap P0.2). It replaces
    /// the C++ `(parent, slotenv, path, box, inputs...)` cons tuple: identity
    /// comes from the allocated id, so two structurally identical wrapper
    /// instances can never collide through hash-consing.
    ///
    /// The token is **not a signal**: it only ever appears as the first child
    /// of `Clocked(env, y)` and must be treated as an opaque annotation by
    /// every signal traversal.
    pub fn clock_env_token(&mut self, domain_id: u32) -> SigId {
        let id_node = self.arena.int(i64::from(domain_id));
        intern_tag(self.arena, SIG_CLOCKENV_TAG, &[id_node])
    }

    #[must_use]
    /// Builds the C++ `sigDoubleClocked(inside, outside, y)` nested shape.
    pub fn double_clocked(
        &mut self,
        inside_clock: SigId,
        outside_clock: SigId,
        sig: SigId,
    ) -> SigId {
        let outer = self.clocked(outside_clock, sig);
        self.clocked(inside_clock, outer)
    }
}

/// Signal structural matcher result, returned by [`match_sig`].
///
/// This enum is the canonical decoded view over signal trees. It exposes child
/// references directly so analysis and lowering passes can recurse without
/// rebuilding temporary wrapper nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SigMatch<'a> {
    Unknown,
    Int(i32),
    Real(f64),
    Input(i32),
    Output(i32, SigId),
    Delay1(SigId),
    Delay(SigId, SigId),
    Prefix(SigId, SigId),
    IntCast(SigId),
    BitCast(SigId),
    FloatCast(SigId),
    Gen(SigId),
    RdTbl(SigId, SigId),
    WrTbl(SigId, SigId, SigId, SigId),
    Select2(SigId, SigId, SigId),
    AssertBounds(SigId, SigId, SigId),
    Lowest(SigId),
    Highest(SigId),
    BinOp(BinOp, SigId, SigId),
    Pow(SigId, SigId),
    Min(SigId, SigId),
    Max(SigId, SigId),
    Acos(SigId),
    Asin(SigId),
    Atan(SigId),
    Atan2(SigId, SigId),
    Cos(SigId),
    Sin(SigId),
    Tan(SigId),
    Exp(SigId),
    Exp10(SigId),
    Log(SigId),
    Log10(SigId),
    Sqrt(SigId),
    Abs(SigId),
    Fmod(SigId, SigId),
    Remainder(SigId, SigId),
    Floor(SigId),
    Ceil(SigId),
    Rint(SigId),
    Round(SigId),
    FFun(SigId, SigId),
    FConst(SigId, SigId, SigId),
    FVar(SigId, SigId, SigId),
    Proj(i32, SigId),
    Rec(SigId),
    ReverseTimeRec(SigId),
    /// General block-reverse-AD carrier. See [`SigBuilder::block_reverse_ad`]
    /// for the semantic contract and on-arena layout.
    ///
    /// `body`, `seeds`, and `cotangents` carry the cons-list head of the
    /// corresponding child list (use `tlib::list_to_vec` to materialise).
    /// `primal_count` is redundant with `body`'s list length but is read
    /// directly during validation and lowering to avoid re-walking the
    /// list. `policy` is decoded from the on-arena `Int` child.
    BlockReverseAD {
        body: SigId,
        primal_count: i32,
        seeds: SigId,
        cotangents: SigId,
        policy: BlockRevPolicy,
    },
    Fir(&'a [SigId]),
    Iir(&'a [SigId]),
    Button(ControlId),
    Checkbox(ControlId),
    VSlider(ControlId),
    HSlider(ControlId),
    NumEntry(ControlId),
    VBargraph(ControlId, SigId),
    HBargraph(ControlId, SigId),
    Attach(SigId, SigId),
    Enable(SigId, SigId),
    Control(SigId, SigId),
    Waveform(&'a [SigId]),
    Soundfile(ControlId),
    SoundfileLength(SigId, SigId),
    SoundfileRate(SigId, SigId),
    SoundfileBuffer(SigId, SigId, SigId, SigId),
    TempVar(SigId),
    PermVar(SigId),
    Seq(SigId, SigId),
    ZeroPad(SigId, SigId),
    OnDemand(&'a [SigId]),
    Upsampling(&'a [SigId]),
    Downsampling(&'a [SigId]),
    Clocked(SigId, SigId),
    /// Opaque clock-environment token (`SIGCLOCKENV`), first child of
    /// `Clocked(env, y)`. Carries the integer id of the propagation-owned
    /// `ClockDomain` side-table entry. Not a signal: traversals must never
    /// recurse into or past it.
    ClockEnvToken(u32),
}

/// Decodes one `SigId` into a canonical [`SigMatch`] shape.
///
/// Accepts encodings produced by this crate and by C++-parity passes.
/// Malformed trees fall back to [`SigMatch::Unknown`].
#[must_use]
pub fn match_sig<'a>(arena: &'a TreeArena, id: SigId) -> SigMatch<'a> {
    let Some(node) = arena.node(id) else {
        return SigMatch::Unknown;
    };
    match &node.kind {
        NodeKind::Int(v) => match i32::try_from(*v) {
            Ok(v) => SigMatch::Int(v),
            Err(_) => SigMatch::Unknown,
        },
        NodeKind::FloatBits(bits) => SigMatch::Real(f64::from_bits(*bits)),
        NodeKind::Tag(tag_id) => {
            let tag = arena.tag_name(*tag_id).unwrap_or("");
            let ch = node.children.as_slice();
            match (tag, ch) {
                (SIG_INPUT_TAG, [idx]) => match arena.kind(*idx) {
                    Some(NodeKind::Int(i)) => match i32::try_from(*i) {
                        Ok(i) => SigMatch::Input(i),
                        Err(_) => SigMatch::Unknown,
                    },
                    _ => SigMatch::Unknown,
                },
                (SIG_OUTPUT_TAG, [idx, s]) => match arena.kind(*idx) {
                    Some(NodeKind::Int(i)) => match i32::try_from(*i) {
                        Ok(i) => SigMatch::Output(i, *s),
                        Err(_) => SigMatch::Unknown,
                    },
                    _ => SigMatch::Unknown,
                },
                (SIG_DELAY1_TAG, [s]) => SigMatch::Delay1(*s),
                (SIG_DELAY_TAG, [s0, s1]) => SigMatch::Delay(*s0, *s1),
                (SIG_PREFIX_TAG, [s0, s1]) => SigMatch::Prefix(*s0, *s1),
                (SIG_INT_CAST_TAG, [x]) => SigMatch::IntCast(*x),
                (SIG_BIT_CAST_TAG, [x]) => SigMatch::BitCast(*x),
                (SIG_FLOAT_CAST_TAG, [x]) => SigMatch::FloatCast(*x),
                (SIG_GEN_TAG, [x]) => SigMatch::Gen(*x),
                (SIG_RDTBL_TAG, [tbl, ri]) => SigMatch::RdTbl(*tbl, *ri),
                (SIG_WRTBL_TAG, [size, generator, wi, ws]) => {
                    SigMatch::WrTbl(*size, *generator, *wi, *ws)
                }
                (SIG_SELECT2_TAG, [selector, s1, s2]) => SigMatch::Select2(*selector, *s1, *s2),
                (SIG_ASSERT_BOUNDS_TAG, [s1, s2, s3]) => SigMatch::AssertBounds(*s1, *s2, *s3),
                (SIG_LOWEST_TAG, [s]) => SigMatch::Lowest(*s),
                (SIG_HIGHEST_TAG, [s]) => SigMatch::Highest(*s),
                (SIG_BINOP_TAG, [op, x, y]) => match arena.kind(*op) {
                    Some(NodeKind::Int(raw)) => match BinOp::from_raw(*raw) {
                        Some(bop) => SigMatch::BinOp(bop, *x, *y),
                        None => SigMatch::Unknown,
                    },
                    _ => SigMatch::Unknown,
                },
                (SIG_POW_TAG, [x, y]) => SigMatch::Pow(*x, *y),
                (SIG_MIN_TAG, [x, y]) => SigMatch::Min(*x, *y),
                (SIG_MAX_TAG, [x, y]) => SigMatch::Max(*x, *y),
                (SIG_ACOS_TAG, [x]) => SigMatch::Acos(*x),
                (SIG_ASIN_TAG, [x]) => SigMatch::Asin(*x),
                (SIG_ATAN_TAG, [x]) => SigMatch::Atan(*x),
                (SIG_ATAN2_TAG, [x, y]) => SigMatch::Atan2(*x, *y),
                (SIG_COS_TAG, [x]) => SigMatch::Cos(*x),
                (SIG_SIN_TAG, [x]) => SigMatch::Sin(*x),
                (SIG_TAN_TAG, [x]) => SigMatch::Tan(*x),
                (SIG_EXP_TAG, [x]) => SigMatch::Exp(*x),
                (SIG_EXP10_TAG, [x]) => SigMatch::Exp10(*x),
                (SIG_LOG_TAG, [x]) => SigMatch::Log(*x),
                (SIG_LOG10_TAG, [x]) => SigMatch::Log10(*x),
                (SIG_SQRT_TAG, [x]) => SigMatch::Sqrt(*x),
                (SIG_ABS_TAG, [x]) => SigMatch::Abs(*x),
                (SIG_FMOD_TAG, [x, y]) => SigMatch::Fmod(*x, *y),
                (SIG_REMAINDER_TAG, [x, y]) => SigMatch::Remainder(*x, *y),
                (SIG_FLOOR_TAG, [x]) => SigMatch::Floor(*x),
                (SIG_CEIL_TAG, [x]) => SigMatch::Ceil(*x),
                (SIG_RINT_TAG, [x]) => SigMatch::Rint(*x),
                (SIG_ROUND_TAG, [x]) => SigMatch::Round(*x),
                (SIG_FFUN_TAG, [ff, largs]) => SigMatch::FFun(*ff, *largs),
                (SIG_FCONST_TAG, [ty, name, file]) => SigMatch::FConst(*ty, *name, *file),
                (SIG_FVAR_TAG, [ty, name, file]) => SigMatch::FVar(*ty, *name, *file),
                (SIG_PROJ_TAG, [idx, group]) => match arena.kind(*idx) {
                    Some(NodeKind::Int(i)) => match i32::try_from(*i) {
                        Ok(i) => SigMatch::Proj(i, *group),
                        Err(_) => SigMatch::Unknown,
                    },
                    _ => SigMatch::Unknown,
                },
                (SIG_REC_TAG, [body]) => SigMatch::Rec(*body),
                (SIG_REVERSE_TIME_REC_TAG, [body]) => SigMatch::ReverseTimeRec(*body),
                (SIG_BLOCK_REVERSE_AD_TAG, [body, primal_count, seeds, cotangents, policy]) => {
                    match (arena.kind(*primal_count), arena.kind(*policy)) {
                        (Some(NodeKind::Int(count)), Some(NodeKind::Int(raw_policy))) => {
                            match (i32::try_from(*count), BlockRevPolicy::from_raw(*raw_policy)) {
                                (Ok(primal_count), Some(policy)) => SigMatch::BlockReverseAD {
                                    body: *body,
                                    primal_count,
                                    seeds: *seeds,
                                    cotangents: *cotangents,
                                    policy,
                                },
                                _ => SigMatch::Unknown,
                            }
                        }
                        _ => SigMatch::Unknown,
                    }
                }
                (SIG_FIR_TAG, sigcoefs) => SigMatch::Fir(sigcoefs),
                (SIG_IIR_TAG, sigcoefs) => SigMatch::Iir(sigcoefs),
                (SIG_BUTTON_TAG, [control]) => {
                    decode_control_id(arena, *control).map_or(SigMatch::Unknown, SigMatch::Button)
                }
                (SIG_CHECKBOX_TAG, [control]) => {
                    decode_control_id(arena, *control).map_or(SigMatch::Unknown, SigMatch::Checkbox)
                }
                (SIG_VSLIDER_TAG, [control]) => {
                    decode_control_id(arena, *control).map_or(SigMatch::Unknown, SigMatch::VSlider)
                }
                (SIG_HSLIDER_TAG, [control]) => {
                    decode_control_id(arena, *control).map_or(SigMatch::Unknown, SigMatch::HSlider)
                }
                (SIG_NUMENTRY_TAG, [control]) => {
                    decode_control_id(arena, *control).map_or(SigMatch::Unknown, SigMatch::NumEntry)
                }
                (SIG_VBARGRAPH_TAG, [control, x]) => decode_control_id(arena, *control)
                    .map_or(SigMatch::Unknown, |control| {
                        SigMatch::VBargraph(control, *x)
                    }),
                (SIG_HBARGRAPH_TAG, [control, x]) => decode_control_id(arena, *control)
                    .map_or(SigMatch::Unknown, |control| {
                        SigMatch::HBargraph(control, *x)
                    }),
                (SIG_ATTACH_TAG, [x, y]) => SigMatch::Attach(*x, *y),
                (SIG_ENABLE_TAG, [x, y]) => SigMatch::Enable(*x, *y),
                (SIG_CONTROL_TAG, [x, y]) => SigMatch::Control(*x, *y),
                (SIG_WAVEFORM_TAG, values) => SigMatch::Waveform(values),
                (SIG_SOUNDFILE_TAG, [control]) => decode_control_id(arena, *control)
                    .map_or(SigMatch::Unknown, SigMatch::Soundfile),
                (SIG_SOUNDFILE_LENGTH_TAG, [soundfile, part]) => {
                    SigMatch::SoundfileLength(*soundfile, *part)
                }
                (SIG_SOUNDFILE_RATE_TAG, [soundfile, part]) => {
                    SigMatch::SoundfileRate(*soundfile, *part)
                }
                (SIG_SOUNDFILE_BUFFER_TAG, [soundfile, chan, part, ridx]) => {
                    SigMatch::SoundfileBuffer(*soundfile, *chan, *part, *ridx)
                }
                (SIG_TEMPVAR_TAG, [x]) => SigMatch::TempVar(*x),
                (SIG_PERMVAR_TAG, [x]) => SigMatch::PermVar(*x),
                (SIG_SEQ_TAG, [x, y]) => SigMatch::Seq(*x, *y),
                (SIG_ZEROPAD_TAG, [x, y]) => SigMatch::ZeroPad(*x, *y),
                (SIG_OD_TAG, sigsubs) => SigMatch::OnDemand(sigsubs),
                (SIG_US_TAG, sigsubs) => SigMatch::Upsampling(sigsubs),
                (SIG_DS_TAG, sigsubs) => SigMatch::Downsampling(sigsubs),
                (SIG_CLOCKED_TAG, [clock, y]) => SigMatch::Clocked(*clock, *y),
                (SIG_CLOCKENV_TAG, [id]) => match arena.kind(*id) {
                    Some(NodeKind::Int(value)) => match u32::try_from(*value) {
                        Ok(value) => SigMatch::ClockEnvToken(value),
                        Err(_) => SigMatch::Unknown,
                    },
                    _ => SigMatch::Unknown,
                },
                _ => SigMatch::Unknown,
            }
        }
        _ => SigMatch::Unknown,
    }
}

/// Deterministic structural dump for signal differential checks.
///
/// Output is shape-and-label based and intentionally excludes arena addresses.
#[must_use]
pub fn dump_sig(arena: &TreeArena, root: SigId) -> String {
    let mut out = String::new();
    dump_node_iter(arena, root, &mut out, false);
    out
}

/// Deterministic structural dump with readable `SIGBINOP` opcode names.
///
/// Keeps the stable dump shape and augments binary-operator nodes with
/// `op=<name> (<symbol>)` metadata.
#[must_use]
pub fn dump_sig_readable(arena: &TreeArena, root: SigId) -> String {
    let mut out = String::new();
    dump_node_iter(arena, root, &mut out, true);
    out
}

/// Interns a tagged signal node with deterministic child ordering.
///
/// This is the shared low-level constructor used by [`SigBuilder`] methods.
/// It mirrors the C++ `tree(tag, ...)` style while ensuring:
/// - tag strings are interned in the arena tag table,
/// - the node itself is hash-consed through [`TreeArena::intern`].
fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[SigId]) -> SigId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}

fn decode_control_id(arena: &TreeArena, node: SigId) -> Option<ControlId> {
    match arena.kind(node) {
        Some(NodeKind::Int(value)) => ControlId::try_from(*value).ok(),
        _ => None,
    }
}

enum DumpTask {
    Node(SigId),
    Static(&'static str),
    Owned(String),
}

fn dump_node_iter(arena: &TreeArena, id: SigId, out: &mut String, readable: bool) {
    let mut stack = vec![DumpTask::Node(id)];
    while let Some(task) = stack.pop() {
        match task {
            DumpTask::Node(id) => {
                let Some(node) = arena.node(id) else {
                    write!(out, "<invalid:{}>", id.as_u32()).expect("String write cannot fail");
                    continue;
                };

                match &node.kind {
                    NodeKind::Nil => out.push_str("nil"),
                    NodeKind::Cons => {
                        stack.push(DumpTask::Static(")"));
                        match node.children.get(1) {
                            Some(tail) => stack.push(DumpTask::Node(tail)),
                            None => stack.push(DumpTask::Static("<missing>")),
                        }
                        stack.push(DumpTask::Static(", "));
                        match node.children.get(0) {
                            Some(head) => stack.push(DumpTask::Node(head)),
                            None => stack.push(DumpTask::Static("<missing>")),
                        }
                        stack.push(DumpTask::Static("cons("));
                    }
                    NodeKind::Symbol(name) => {
                        write!(out, "sym({name:?})").expect("String write cannot fail");
                    }
                    NodeKind::StringLiteral(value) => {
                        write!(out, "str({value:?})").expect("String write cannot fail");
                    }
                    NodeKind::Int(value) => {
                        write!(out, "int({value})").expect("String write cannot fail");
                    }
                    NodeKind::FloatBits(bits) => {
                        write!(out, "float_bits(0x{bits:016x})").expect("String write cannot fail");
                    }
                    NodeKind::Tag(tag_id) => {
                        let tag_name = arena.tag_name(*tag_id).unwrap_or("<unknown-tag>");
                        if readable && tag_name == SIG_BINOP_TAG && node.children.len() == 3 {
                            let op_id = node.children.get(0).unwrap_or_else(|| arena.nil());
                            let x_id = node.children.get(1).unwrap_or_else(|| arena.nil());
                            let y_id = node.children.get(2).unwrap_or_else(|| arena.nil());
                            let op_desc = match arena.kind(op_id) {
                                Some(NodeKind::Int(raw)) => match BinOp::from_raw(*raw) {
                                    Some(op) => format!("{} ({})", op.name(), op.symbol()),
                                    None => format!("unknown({raw})"),
                                },
                                _ => "unknown".to_owned(),
                            };
                            stack.push(DumpTask::Static(")"));
                            stack.push(DumpTask::Node(y_id));
                            stack.push(DumpTask::Static(", "));
                            stack.push(DumpTask::Node(x_id));
                            stack.push(DumpTask::Owned(format!("{SIG_BINOP_TAG}(op={op_desc}, ")));
                            continue;
                        }

                        stack.push(DumpTask::Static(")"));
                        for (idx, child) in node.children.as_slice().iter().enumerate().rev() {
                            stack.push(DumpTask::Node(*child));
                            if idx > 0 {
                                stack.push(DumpTask::Static(", "));
                            }
                        }
                        stack.push(DumpTask::Owned(format!("{tag_name}(")));
                    }
                }
            }
            DumpTask::Static(text) => out.push_str(text),
            DumpTask::Owned(text) => out.push_str(&text),
        }
    }
}
