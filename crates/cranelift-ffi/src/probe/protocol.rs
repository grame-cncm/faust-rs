//! The reference impulse-test protocol, and the `.ir` text format.
//!
//! # Why the probe reproduces it
//! `faustprobe` exists to vary things, which is the opposite of what a
//! regression corpus needs. But the two jobs share a runtime, and a tool that
//! could not also emit the reference format would force every regression run
//! through a second binary. `--protocol impulse-test` pins every knob to the
//! reference values and emits the same text, so the probe subsumes
//! `impulse-cranelift` rather than competing with it.
//!
//! The protocol is defined by `controlTools.h::runDSP` in the C++ test suite
//! and mirrored by both existing runners:
//!
//! - sample rate 44100, block size 64;
//! - first frame of every input channel is 1.0, everything else 0.0;
//! - every `button` zone held at 1.0 for the first block, then 0.0
//!   (`FUI::setButtons` drives buttons only — checkboxes and sliders keep
//!   their declared defaults);
//! - samples printed as `"%6d :  %8.6f ..."` after a zero-clamp of values
//!   below 1e-6 in magnitude.
//!
//! Only the scalar pass is produced. The C++ reference is four passes, so the
//! comparison against it uses `filesCompare -part`, which compares the prefix
//! — exactly how the C++ suite tests its own scalar-only Rust architecture.

/// Reference sample rate.
pub const SAMPLE_RATE: i32 = 44_100;
/// Reference block size.
pub const BLOCK_SIZE: usize = 64;
/// Scalar-pass frame count: the C++ reference's `nbsamples / 4` with
/// `nbsamples == 60000`.
pub const DEFAULT_FRAMES: usize = 15_000;

/// Zero-clamp applied before printing.
///
/// Mirrors `controlTools.h::normalize`. Non-finite values pass through
/// unchanged so a divergence stays visible in the `.ir` rather than being
/// silently flattened to zero.
#[must_use]
pub fn normalize(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        value
    } else if value.abs() < 0.000_001 {
        0.0
    } else {
        value
    }
}

/// Format the `.ir` header.
#[must_use]
pub fn header(inputs: usize, outputs: usize, frames: usize) -> String {
    format!(
        "number_of_inputs  : {inputs:3}\nnumber_of_outputs : {outputs:3}\nnumber_of_frames  : {frames:6}\n"
    )
}

/// Format one `.ir` sample line.
#[must_use]
pub fn frame_line(frame: usize, samples: &[f64]) -> String {
    let mut line = format!("{frame:6} : ");
    for value in samples {
        line.push_str(&format!(" {:8.6}", normalize(*value)));
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_below_one_microunit() {
        assert!((normalize(9.9e-7)).abs() < f64::EPSILON);
        assert!((normalize(1.1e-6) - 1.1e-6).abs() < f64::EPSILON);
        assert!((normalize(-9.9e-7)).abs() < f64::EPSILON);
    }

    #[test]
    fn preserves_non_finite() {
        // A NaN in the reference output is a signal, not noise: flattening it
        // to zero would hide a divergence from the comparison.
        assert!(normalize(f64::NAN).is_nan());
        assert!(normalize(f64::INFINITY).is_infinite());
    }

    #[test]
    fn header_matches_reference_widths() {
        assert_eq!(
            header(1, 2, 15_000),
            "number_of_inputs  :   1\nnumber_of_outputs :   2\nnumber_of_frames  :  15000\n"
        );
    }

    #[test]
    fn frame_line_matches_reference_widths() {
        assert_eq!(
            frame_line(0, &[0.5, -0.25]),
            "     0 :  0.500000 -0.250000\n"
        );
    }

    #[test]
    fn frame_line_applies_the_clamp() {
        assert_eq!(frame_line(7, &[1e-9]), "     7 :  0.000000\n");
    }
}
