//! How the probe writes a number.
//!
//! The probe is read by scripts that compare its output with a reference, so
//! the text of a number is part of the measurement. Nine fixed decimals, the
//! first choice, loses it twice: seven of a double's sixteen digits near 1,
//! and nearly everything of a small value (`3.3e-8` prints `0.000000033`, two
//! significant digits, in either width), which is the tail of a reverberation,
//! a gradient, a residual: what one inspects when something is subtly wrong.
//!
//! The default is therefore the **shortest text that parses back to the same
//! float, in the width the program was compiled in**: a sample of a
//! single-precision program is formatted from the `f32`, since its `f64`
//! conversion needs seventeen digits to say what nine say
//! (`0.0010000000474974513` for `0.001`). Plain or scientific, whichever Rust
//! picks for the magnitude. `Fixed(n)` keeps the old form, and `Fixed(9)` the
//! old bytes, for anything pinned to them.

/// The text form of the numbers the probe prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    /// Shortest text that round-trips at the value's own width.
    #[default]
    RoundTrip,
    /// This many fixed decimals (`{:.N}`), scientific for a training loss.
    Fixed(usize),
}

impl Precision {
    /// Parses `--precision`: `full` or a number of decimals.
    ///
    /// # Errors
    /// Names the value when it is neither.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text == "full" {
            return Ok(Self::RoundTrip);
        }
        text.parse()
            .map(Self::Fixed)
            .map_err(|_| format!("`--precision {text}`: expected `full` or a number of decimals"))
    }
}

/// A [`Precision`] bound to the width of the program's samples.
#[derive(Debug, Clone, Copy)]
pub struct NumberFormat {
    precision: Precision,
    double: bool,
}

impl NumberFormat {
    /// The format of a program compiled in double (`true`) or single precision.
    #[must_use]
    pub const fn new(precision: Precision, double: bool) -> Self {
        Self { precision, double }
    }

    /// A value that lives at the program's width: a sample, a peak (which is a
    /// sample's magnitude), a bargraph, a control's bound.
    #[must_use]
    pub fn sample(&self, value: f64) -> String {
        match self.precision {
            Precision::Fixed(decimals) => format!("{value:.decimals$}"),
            Precision::RoundTrip if self.double => format!("{value:?}"),
            // exact: the value came from an `f32`
            Precision::RoundTrip => format!("{:?}", value as f32),
        }
    }

    /// A value computed in `f64` whatever the program's width: a mean, an RMS,
    /// a reduction, a trained control.
    #[must_use]
    pub fn computed(&self, value: f64) -> String {
        match self.precision {
            Precision::Fixed(decimals) => format!("{value:.decimals$}"),
            Precision::RoundTrip => format!("{value:?}"),
        }
    }

    /// A training loss: always scientific, since it spans many decades.
    #[must_use]
    pub fn loss(&self, value: f64) -> String {
        match self.precision {
            Precision::Fixed(decimals) => format!("{value:.decimals$e}"),
            Precision::RoundTrip => format!("{value:e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NumberFormat, Precision};

    #[test]
    fn round_trip_text_parses_back_to_the_same_bits() {
        let double = NumberFormat::new(Precision::RoundTrip, true);
        for value in [
            1.0 / 3.0,
            1.0e-7 / 3.0,
            std::f64::consts::PI,
            -2.5e-300,
            6.02e23,
            0.1,
            0.0,
        ] {
            let text = double.sample(value);
            assert_eq!(
                text.parse::<f64>().unwrap().to_bits(),
                value.to_bits(),
                "{text}"
            );
        }
        let single = NumberFormat::new(Precision::RoundTrip, false);
        for value in [1.0_f32 / 3.0, 1.0e-7 / 3.0, 0.001, -7.25e-20, 3.0e30] {
            let text = single.sample(f64::from(value));
            assert_eq!(
                text.parse::<f32>().unwrap().to_bits(),
                value.to_bits(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_single_precision_value_is_not_printed_through_its_double() {
        let single = NumberFormat::new(Precision::RoundTrip, false);
        assert_eq!(single.sample(f64::from(0.001_f32)), "0.001");
        // the same number as a computed f64 is the double it is
        assert_eq!(
            single.computed(f64::from(0.001_f32)),
            "0.0010000000474974513"
        );
    }

    #[test]
    fn a_small_value_keeps_its_digits() {
        let double = NumberFormat::new(Precision::RoundTrip, true);
        assert_eq!(double.sample(1.0e-7 / 3.0), "3.3333333333333334e-8");
        // what nine fixed decimals made of it
        assert_eq!(
            NumberFormat::new(Precision::Fixed(9), true).sample(1.0e-7 / 3.0),
            "0.000000033"
        );
    }

    #[test]
    fn fixed_nine_is_the_old_text() {
        let old = NumberFormat::new(Precision::Fixed(9), false);
        assert_eq!(old.sample(0.5), "0.500000000");
        assert_eq!(old.computed(-0.25), "-0.250000000");
        assert_eq!(old.loss(2.388_789_544e-4), "2.388789544e-4");
        assert_eq!(old.sample(f64::NAN), "NaN");
    }

    #[test]
    fn non_finite_values_have_a_text_python_reads() {
        let double = NumberFormat::new(Precision::RoundTrip, true);
        assert_eq!(double.sample(f64::NAN), "NaN");
        assert_eq!(double.sample(f64::INFINITY), "inf");
        assert_eq!(double.sample(f64::NEG_INFINITY), "-inf");
    }

    #[test]
    fn precision_parses_full_or_a_count() {
        assert_eq!(Precision::parse("full").unwrap(), Precision::RoundTrip);
        assert_eq!(Precision::parse("9").unwrap(), Precision::Fixed(9));
        assert!(Precision::parse("many").is_err());
    }
}
