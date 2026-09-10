//! What the block reverse sweep of `rad` records in tapes.
//!
//! A tape holds one forward value per sample of the block, so the tapes are
//! the memory of a `rad` over a long block (`-bra-tape`), and their stores
//! and loads a share of its time. Two things keep their number down: a value
//! that is constant over the block (a filter coefficient computed from
//! sliders and the sample rate, `ma.SR` being `min(192000, max(1,
//! fconstant(...)))`) is re-evaluated in the reverse loop rather than taped;
//! and two signals that lower to the same FIR value share one tape.

use compiler::{Compiler, SignalFirLane};
use std::collections::BTreeSet;

/// The distinct `fBraTape*` fields of the FIR of `source`.
fn tape_count(name: &str, source: &str) -> usize {
    let fir = Compiler::new()
        .compile_source_to_fir_with_lane(name, source, SignalFirLane::TransformFastLane)
        .unwrap_or_else(|e| panic!("{name}: FIR lowering failed: {e}"));
    let text = fir::dump_fir(&fir.store, fir.module);
    let mut names = BTreeSet::new();
    let mut rest = text.as_str();
    while let Some(pos) = rest.find("fBraTape") {
        let tail = &rest[pos + "fBraTape".len()..];
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        names.insert(digits);
        rest = tail;
    }
    names.len()
}

/// `y = c (x + g y')` with `c = cos(2 pi fc / SR)`: the sweep needs `y'`
/// (for `g`) and `x + g y'` (for `c`), two values that change every sample.
/// The coefficient `c` and everything under it are constant over the block
/// and are not taped, although `SR` goes through `min`/`max`.
#[test]
fn a_coefficient_computed_from_the_clamped_sample_rate_is_not_taped() {
    let source = r#"
SR = min(192000.0, max(1.0, fconstant(int fSamplingFreq, <math.h>)));
fc = hslider("fc", 1000.0, 20.0, 20000.0, 1.0);
g = hslider("g", 0.5, -0.9, 0.9, 0.001);
c = cos(6.283185307179586 * fc / SR);
process = rad((_ : (+ : *(c)) ~ *(g)), (fc, g));
"#;
    assert_eq!(tape_count("clamped_sr_coefficient.dsp", source), 2);
}

/// The recursion's previous output is read inside the body (through the
/// recursion's own back-reference) and outside it (as the delayed output):
/// the two signals lower to the same recursion slot and share one tape.
#[test]
fn a_value_read_inside_and_outside_its_recursion_gets_one_tape() {
    let source = r#"
g = hslider("g", 0.5, -0.9, 0.9, 0.001);
process = rad(((_ : + ~ *(g)) <: _, _') : *, g);
"#;
    // `y'` (inside, for `g`; outside, for `y * y'`) and `y` (outside).
    assert_eq!(tape_count("shared_recursion_slot.dsp", source), 2);
}
