//! `coverage` group of the signal_fir lowering tests (split from the former
//! monolithic `tests.rs`; test names unchanged).

use super::fixtures::*;
use crate::signal_fir::{SignalFirErrorCode, SignalFirOptions};
use signals::{BinOp, SigBuilder};
use tlib::TreeArena;

// ── Lowering-coverage obligation (rewriting-calculus §8.2 / analysis W8) ──────
//
// The signal→FIR rewriting-calculus formalisation states the obligation
// `L_prep ⊆ dom(lower)`: every signal constructor that `signal_prepare::verify`
// accepts must be lowerable by `lower_signal`. These tests make the obligation
// executable. See `porting/signal-to-fir-rewriting-calculus-2026-06-20-en.md`.

use signals::{SigMatch, match_sig};
/// Whether the fast-lane `lower_signal` dispatcher handles a constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoweringCoverage {
    /// Has its own arm in `lower_signal` (lowered when encountered directly).
    Direct,
    /// No top-level arm, but reachable through a parent arm (currently `Proj`).
    ViaParent,
    /// Falls through to `other => UnsupportedSignalNode`.
    Unsupported,
    /// Has its own arm in `lower_signal` that returns the dedicated
    /// `ClockedNotLowered` (`FRS-SFIR-0007`) rejection: clocked machinery
    /// accepted by `signal_prepare`, rejected when no clocked-lowering state
    /// is active for the run.
    ClockedRejection,
}
/// Exhaustive classification of every `SigMatch` constructor against the
/// `lower_signal` dispatcher in `module/core_lowering.rs`.
///
/// This match intentionally has **no wildcard arm**: a newly added `SigMatch`
/// variant will fail to compile here until it is consciously classified, which
/// is the drift guard for the `L_prep ⊆ dom(lower)` coverage obligation. The
/// `Unsupported` arm is the executable record of the W8 gap (families that
/// `signal_prepare::verify` accepts but `lower_signal` does not handle).
fn lowering_coverage(m: &SigMatch<'_>) -> LoweringCoverage {
    use LoweringCoverage::{ClockedRejection, Direct, Unsupported, ViaParent};
    match m {
        SigMatch::Int(_)
        | SigMatch::Real(_)
        | SigMatch::Input(_)
        | SigMatch::Output(..)
        | SigMatch::Delay1(_)
        | SigMatch::Delay(..)
        | SigMatch::Prefix(..)
        | SigMatch::IntCast(_)
        | SigMatch::BitCast(_)
        | SigMatch::FloatCast(_)
        | SigMatch::Select2(..)
        | SigMatch::Proj(..)
        | SigMatch::BinOp(..)
        | SigMatch::Pow(..)
        | SigMatch::Min(..)
        | SigMatch::Max(..)
        | SigMatch::Acos(_)
        | SigMatch::Asin(_)
        | SigMatch::Atan(_)
        | SigMatch::Atan2(..)
        | SigMatch::Cos(_)
        | SigMatch::Sin(_)
        | SigMatch::Tan(_)
        | SigMatch::Exp(_)
        | SigMatch::Exp10(_)
        | SigMatch::Log(_)
        | SigMatch::Log10(_)
        | SigMatch::Sqrt(_)
        | SigMatch::Abs(_)
        | SigMatch::Fmod(..)
        | SigMatch::Remainder(..)
        | SigMatch::Floor(_)
        | SigMatch::Ceil(_)
        | SigMatch::Rint(_)
        | SigMatch::Round(_)
        | SigMatch::Lowest(_)
        | SigMatch::Highest(_)
        | SigMatch::FConst(..)
        | SigMatch::FVar(..)
        | SigMatch::FFun(..)
        | SigMatch::RdTbl(..)
        | SigMatch::WrTbl(..)
        | SigMatch::Waveform(_)
        | SigMatch::Button(_)
        | SigMatch::Checkbox(_)
        | SigMatch::VSlider(_)
        | SigMatch::HSlider(_)
        | SigMatch::NumEntry(_)
        | SigMatch::VBargraph(..)
        | SigMatch::HBargraph(..)
        | SigMatch::Attach(..)
        | SigMatch::Enable(..)
        | SigMatch::Control(..)
        | SigMatch::Soundfile(_)
        | SigMatch::SoundfileLength(..)
        | SigMatch::SoundfileRate(..)
        | SigMatch::SoundfileBuffer(..) => Direct,

        // Reverse-mode-AD carriers: no top-level arm; lowered through `Proj`.
        SigMatch::BlockReverseAD { .. } | SigMatch::ReverseTimeRec(_) => ViaParent,

        // Clocked machinery: dedicated structured rejection when no
        // clocked-lowering state is active.
        SigMatch::TempVar(_)
        | SigMatch::PermVar(_)
        | SigMatch::Seq(..)
        | SigMatch::ZeroPad(..)
        | SigMatch::Clocked(..)
        | SigMatch::ClockEnvToken(_)
        | SigMatch::OnDemand(_)
        | SigMatch::Upsampling(_)
        | SigMatch::Downsampling(_) => ClockedRejection,

        // Accepted by `verify` but with no `lower_signal` arm — the W8 gap.
        SigMatch::Gen(_)
        | SigMatch::AssertBounds(..)
        | SigMatch::Fir(_)
        | SigMatch::Iir(_)
        // Pre-preparation / legacy forms that `verify` itself also rejects.
        | SigMatch::Unknown
        | SigMatch::Rec(_) => Unsupported,
    }
}
#[test]
fn lowering_coverage_classifies_sampled_families() {
    let mut arena = TreeArena::new();
    let (int_sig, binop_sig, sin_sig, gen_sig) = {
        let mut b = SigBuilder::new(&mut arena);
        let i = b.int(1);
        let c = b.real(0.5);
        let binop = b.binop(BinOp::Mul, i, c);
        let sin = b.sin(c);
        let r = b.real(1.0);
        let gen_node = b.generate(r);
        (i, binop, sin, gen_node)
    };
    assert_eq!(
        lowering_coverage(&match_sig(&arena, int_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, binop_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, sin_sig)),
        LoweringCoverage::Direct
    );
    assert_eq!(
        lowering_coverage(&match_sig(&arena, gen_sig)),
        LoweringCoverage::Unsupported
    );
}
#[test]
fn fastlane_lowers_representative_supported_families() {
    // A small program touching several Direct families: Input, Real, BinOp, Sin.
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let x = b.input(0);
        let g = b.real(0.5);
        let scaled = b.binop(BinOp::Mul, x, g);
        b.sin(scaled)
    };
    compile_fastlane_without_ui(&arena, &[sig], 1, 1, &SignalFirOptions::default())
        .expect("supported Direct families must lower to a FIR module");
}
#[test]
fn fastlane_rejects_unsupported_family_with_typed_error() {
    // `Gen` is accepted by `signal_prepare::verify` but has no `lower_signal`
    // arm (the W8 coverage gap). Compilation must fail with the stable typed
    // code rather than silently emit a module. Whether the rejection happens at
    // preparation or at lowering, the surfaced code is `UnsupportedSignalNode`.
    let mut arena = TreeArena::new();
    let sig = {
        let mut b = SigBuilder::new(&mut arena);
        let r = b.real(1.0);
        b.generate(r)
    };
    let err = compile_fastlane_without_ui(&arena, &[sig], 0, 1, &SignalFirOptions::default())
        .expect_err("an unsupported signal family must not silently compile");
    assert_eq!(err.code(), SignalFirErrorCode::UnsupportedSignalNode);
}
