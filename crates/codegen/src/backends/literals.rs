//! Literal spelling shared by the textual backends: real numbers and string
//! escapes.
//!
//! # Why this exists
//! Upstream `compiler/generator/Text.hh` formats literals once for every text
//! backend. The port had seven real-number formatters and seven string
//! escapers, one per backend, and the copies drifted: the `-0.0` normalization
//! reached four of them, the NaN/Inf special case six, the `\r`/`\t` escapes
//! five (`porting/codegen-factorization-analysis-2026-10-18-en.md`, R2). The
//! algorithm lives here once; what legitimately differs per target — how NaN
//! and infinities are spelled, Rust's `{:?}` digits, whether `-0.0` survives —
//! is a [`RealSpelling`] descriptor each backend owns as a `const`.
//!
//! # Invariant
//! A descriptor reproduces its backend's output exactly. Changing one is a
//! change of generated code, made in its own commit with its own test.

/// How the digits of a finite real are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Digits {
    /// Rust `Display` — the shortest text that reads back as the same value,
    /// never an exponent — plus `.0` when it has no point: `0.5`, `3.0`,
    /// `0.0000000001`, `1e23` as `100000000000000000000000.0`.
    Shortest,
    /// As [`Digits::Shortest`], except that an integral value is written
    /// with all of its exact digits (`{:.1}`): `1e23` as
    /// `99999999999999991611392.0`. Same number, longer text.
    ExactIntegral,
    /// Rust `Debug`: the shortest round-trip text, `.0` kept, exponent for
    /// very large or small values (`1e-10`).
    Debug,
}

/// Whether a negative zero keeps its sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NegativeZero {
    /// `-0.0` is written `0.0`, as upstream does for a folded negative zero.
    Normalize,
    /// `-0.0` is written as is.
    Keep,
}

/// One target's spelling of real literals.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RealSpelling {
    pub(crate) nan: &'static str,
    pub(crate) pos_inf: &'static str,
    pub(crate) neg_inf: &'static str,
    pub(crate) digits: Digits,
    pub(crate) negative_zero: NegativeZero,
}

impl RealSpelling {
    /// Formats `value` at double precision, without a type suffix.
    pub(crate) fn format(&self, value: f64) -> String {
        if let Some(special) = self.special(value) {
            return special.to_owned();
        }
        let text = match self.digits {
            Digits::Shortest => with_point(format!("{value}")),
            Digits::ExactIntegral if value == value.trunc() => format!("{value:.1}"),
            Digits::ExactIntegral => format!("{value}"),
            Digits::Debug => format!("{value:?}"),
        };
        self.zero(text)
    }

    /// Formats `value` narrowed to single precision first, so `0.3` reads
    /// `0.3` rather than `0.30000001192092896`. A special value is spelled
    /// from the unnarrowed value; a finite value beyond `f32::MAX` narrows to
    /// an infinity written from `f32`'s own `Display` (`inf`, which
    /// [`Digits::Shortest`] then suffixes with `.0`).
    pub(crate) fn format_narrowed(&self, value: f64) -> String {
        if let Some(special) = self.special(value) {
            return special.to_owned();
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "single-precision literals narrow to f32 by definition"
        )]
        let narrowed = value as f32;
        let text = match self.digits {
            Digits::Shortest => with_point(format!("{narrowed}")),
            Digits::ExactIntegral if narrowed.is_finite() && narrowed == narrowed.trunc() => {
                format!("{narrowed:.1}")
            }
            Digits::ExactIntegral => format!("{narrowed}"),
            Digits::Debug => format!("{narrowed:?}"),
        };
        self.zero(text)
    }

    fn special(&self, value: f64) -> Option<&'static str> {
        if value.is_nan() {
            Some(self.nan)
        } else if value.is_infinite() {
            Some(if value.is_sign_negative() {
                self.neg_inf
            } else {
                self.pos_inf
            })
        } else {
            None
        }
    }

    fn zero(&self, text: String) -> String {
        if self.negative_zero == NegativeZero::Normalize && text == "-0.0" {
            "0.0".to_owned()
        } else {
            text
        }
    }
}

/// Appends `.0` to digits that have neither a point nor an exponent, so the
/// target reads a real rather than an integer.
fn with_point(mut text: String) -> String {
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

/// Which characters a string literal escapes beyond `\\ " \n \r \t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StringDialect {
    /// C, C++, Rust, Cmajor, AssemblyScript: the five base escapes.
    C,
    /// Julia also escapes `$`, which it would otherwise interpolate.
    Julia,
    /// JSON also escapes every other control character as `\uXXXX`.
    Json,
}

/// Escapes `input` for the inside of a double-quoted literal.
pub(crate) fn escape_string(input: &str, dialect: StringDialect) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if dialect == StringDialect::Julia => out.push_str("\\$"),
            c if dialect == StringDialect::Json && c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// `input` as a double-quoted literal.
pub(crate) fn quoted_string(input: &str, dialect: StringDialect) -> String {
    format!("\"{}\"", escape_string(input, dialect))
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: RealSpelling = RealSpelling {
        nan: "NAN",
        pos_inf: "INFINITY",
        neg_inf: "-INFINITY",
        digits: Digits::Shortest,
        negative_zero: NegativeZero::Normalize,
    };
    const EXACT: RealSpelling = RealSpelling {
        nan: "nan",
        pos_inf: "inf",
        neg_inf: "-inf",
        digits: Digits::ExactIntegral,
        negative_zero: NegativeZero::Keep,
    };
    const KEEP_DEBUG: RealSpelling = RealSpelling {
        nan: "nan",
        pos_inf: "inf",
        neg_inf: "-inf",
        digits: Digits::Debug,
        negative_zero: NegativeZero::Keep,
    };

    #[test]
    fn display_digits_add_a_point_and_never_an_exponent() {
        assert_eq!(C.format(3.0), "3.0");
        assert_eq!(C.format(0.5), "0.5");
        assert_eq!(C.format(1e-10), "0.0000000001");
        assert_eq!(C.format(-0.0), "0.0");
        assert_eq!(C.format(f64::NAN), "NAN");
        assert_eq!(C.format(f64::NEG_INFINITY), "-INFINITY");
    }

    #[test]
    fn exact_integral_digits_differ_only_on_large_integers() {
        assert_eq!(C.format(1e23), "100000000000000000000000.0");
        assert_eq!(EXACT.format(1e23), "99999999999999991611392.0");
        assert_eq!(EXACT.format(3.0), "3.0");
        assert_eq!(EXACT.format(0.5), "0.5");
        assert_eq!(EXACT.format(-0.0), "-0.0");
        assert_eq!(EXACT.format_narrowed(1e300), "inf");
    }

    #[test]
    fn debug_digits_and_kept_negative_zero() {
        assert_eq!(KEEP_DEBUG.format(1e-10), "1e-10");
        assert_eq!(KEEP_DEBUG.format(-0.0), "-0.0");
        assert_eq!(KEEP_DEBUG.format(3.0), "3.0");
    }

    #[test]
    fn narrowing_formats_in_single_precision() {
        assert_eq!(C.format(f64::from(0.3_f32)), "0.30000001192092896");
        assert_eq!(C.format_narrowed(f64::from(0.3_f32)), "0.3");
        assert_eq!(C.format_narrowed(1e300), "inf.0");
        assert_eq!(C.format_narrowed(f64::INFINITY), "INFINITY");
        assert_eq!(C.format_narrowed(-0.0), "0.0");
    }

    /// Every backend's spelling of the values the copies disagreed on. A row
    /// that differs from its neighbours is a target difference or a drift;
    /// changing one is a change of generated code.
    #[test]
    fn backend_spellings_of_special_values() {
        use crate::backends::{asc, c_family, cmajor, codebox, julia, rust};
        let spellings = [
            ("c/cpp", c_family::C_FAMILY_REAL),
            ("asc", asc::ASC_REAL),
            ("rust", rust::RUST_REAL),
            ("julia", julia::JULIA_REAL),
            ("cmajor", cmajor::CMAJOR_REAL),
            ("codebox", codebox::CODEBOX_REAL),
        ];
        let rows: Vec<String> = spellings
            .iter()
            .map(|(name, spelling)| {
                let cells: Vec<String> = [
                    f64::NAN,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    -0.0,
                    1e-10,
                    1e23,
                ]
                .iter()
                .map(|&v| spelling.format(v))
                .collect();
                format!("{name}: {}", cells.join(" | "))
            })
            .collect();
        assert_eq!(
            rows,
            [
                "c/cpp: NAN | INFINITY | -INFINITY | 0.0 | 0.0000000001 | 100000000000000000000000.0",
                "asc: NaN | Infinity | -Infinity | 0.0 | 0.0000000001 | 100000000000000000000000.0",
                "rust: f64::NAN | f64::INFINITY | f64::NEG_INFINITY | 0.0 | 1e-10 | 1e23",
                "julia: NaN | Inf | -Inf | 0.0 | 1e-10 | 1e23",
                "cmajor: nan | inf | -inf | 0.0 | 0.0000000001 | 99999999999999991611392.0",
                "codebox: NaN | inf | -inf | 0.0 | 0.0000000001 | 99999999999999991611392.0",
            ]
        );
    }

    #[test]
    fn string_dialects_escape_their_extra_characters() {
        let input = "a\\b\"c\nd\re\tf$g\u{1}";
        assert_eq!(
            escape_string(input, StringDialect::C),
            "a\\\\b\\\"c\\nd\\re\\tf$g\u{1}"
        );
        assert_eq!(
            escape_string(input, StringDialect::Julia),
            "a\\\\b\\\"c\\nd\\re\\tf\\$g\u{1}"
        );
        assert_eq!(
            escape_string(input, StringDialect::Json),
            "a\\\\b\\\"c\\nd\\re\\tf$g\\u0001"
        );
        assert_eq!(quoted_string("x", StringDialect::C), "\"x\"");
    }
}
