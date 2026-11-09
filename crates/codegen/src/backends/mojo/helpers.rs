//! The helpers written at the top of a Mojo unit when the emitted code calls
//! them: the C semantics Mojo's operators do not have (`faust_idiv`, ...),
//! and the decoders of packed tables (`MojoOptions::packed_tables`, plan
//! `porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §6).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::{Emitter, PACKED_TABLE_MIN_ENTRIES};
use fir::{FirId, FirMatch, FirType, match_fir};

/// A C-semantics helper written at the top of the unit when used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Helper {
    IDiv,
    IRem,
    FMod,
    Round,
    Unpack32,
    Unpack64,
}

impl Helper {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::IDiv => "faust_idiv",
            Self::IRem => "faust_irem",
            Self::FMod => "faust_fmod",
            Self::Round => "faust_round",
            Self::Unpack32 => "faust_unpack32",
            Self::Unpack64 => "faust_unpack64",
        }
    }

    fn source(self) -> &'static str {
        match self {
            // C truncates the quotient toward zero; Mojo `//` floors.
            Self::IDiv => {
                "@always_inline\n\
                 def faust_idiv[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var q = a // b\n\
                 \x20   if (a % b != 0) and ((a < 0) != (b < 0)):\n\
                 \x20       q += 1\n\
                 \x20   return q\n"
            }
            // C gives the remainder the sign of the dividend; Mojo `%` gives
            // it the sign of the divisor.
            Self::IRem => {
                "@always_inline\n\
                 def faust_irem[dt: DType](a: Scalar[dt], b: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var r = a % b\n\
                 \x20   if (r != 0) and ((r < 0) != (a < 0)):\n\
                 \x20       r -= b\n\
                 \x20   return r\n"
            }
            // C `fmod`: the sign of the dividend. Mojo `%` floors, which is
            // C `fmod` on two non-negative operands, so only the sign is
            // restored.
            Self::FMod => {
                "@always_inline\n\
                 def faust_fmod[dt: DType](x: Scalar[dt], y: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   return copysign(abs(x) % abs(y), x)\n"
            }
            // C `round`: halves away from zero; Mojo `round` goes to even.
            // `x - trunc(x)` is exact, so the test on the half is exact.
            Self::Round => {
                "@always_inline\n\
                 def faust_round[dt: DType](x: Scalar[dt]) -> Scalar[dt]:\n\
                 \x20   var t = trunc(x)\n\
                 \x20   if abs(x - t) >= 0.5:\n\
                 \x20       t += copysign(Scalar[dt](1), x)\n\
                 \x20   return t\n"
            }
            // A packed table (`MojoOptions::packed_tables`): the bit pattern of
            // each entry as 8 (or 16) lowercase hexadecimal digits, most
            // significant first. Mojo compiles the string at once where it
            // takes a minute for a 12,000-entry array literal.
            Self::Unpack32 => {
                "def faust_unpack32[dt: DType, n: Int](hex: StaticString) -> Arr[Scalar[dt], n]:\n\
                 \x20   var out = Arr[Scalar[dt], n](fill=Scalar[dt](0))\n\
                 \x20   var p = hex.unsafe_ptr()\n\
                 \x20   for i in range(n):\n\
                 \x20       var bits = UInt32(0)\n\
                 \x20       for j in range(8):\n\
                 \x20           var c = UInt32(p[i * 8 + j])\n\
                 \x20           bits = (bits << 4) | (c - 48 if c < 58 else c - 87)\n\
                 \x20       out[i] = bitcast[dt](bits)\n\
                 \x20   return out^\n"
            }
            Self::Unpack64 => {
                "def faust_unpack64[dt: DType, n: Int](hex: StaticString) -> Arr[Scalar[dt], n]:\n\
                 \x20   var out = Arr[Scalar[dt], n](fill=Scalar[dt](0))\n\
                 \x20   var p = hex.unsafe_ptr()\n\
                 \x20   for i in range(n):\n\
                 \x20       var bits = UInt64(0)\n\
                 \x20       for j in range(16):\n\
                 \x20           var c = UInt64(p[i * 16 + j])\n\
                 \x20           bits = (bits << 4) | (c - 48 if c < 58 else c - 87)\n\
                 \x20       out[i] = bitcast[dt](bits)\n\
                 \x20   return out^\n"
            }
        }
    }
}

pub(super) fn emit_helpers(out: &mut String, helpers: &BTreeSet<Helper>) {
    for helper in helpers {
        out.push_str(helper.source());
        out.push('\n');
    }
}

impl Emitter<'_> {
    /// A struct table as a call of `faust_unpack32`/`faust_unpack64` on the
    /// bit patterns of its entries (`MojoOptions::packed_tables`), or `None`
    /// when it stays a literal: too short, another element type, or an entry
    /// that is not a literal. An entry's bits are those of the value the
    /// literal would spell: converted to the element type, a negative zero
    /// written as zero (`NegativeZero::Normalize`), a NaN as the canonical
    /// one.
    pub(super) fn packed_table(&mut self, elem_type: &FirType, values: &[FirId]) -> Option<String> {
        if !self.options.packed_tables || values.len() < PACKED_TABLE_MIN_ENTRIES {
            return None;
        }
        let (helper, digits, dtype) = match elem_type {
            FirType::Int32 => (Helper::Unpack32, 8, "s32"),
            FirType::Float32 => (Helper::Unpack32, 8, "f32"),
            FirType::Float64 => (Helper::Unpack64, 16, "f64"),
            _ => return None,
        };
        let mut hex = String::with_capacity(values.len() * digits);
        for &value in values {
            let real = match match_fir(self.store, value) {
                FirMatch::Int32 { value, .. } if *elem_type == FirType::Int32 => {
                    let _ = write!(hex, "{:08x}", value.cast_unsigned());
                    continue;
                }
                FirMatch::Int32 { value, .. } => f64::from(value),
                FirMatch::Float32 { value, .. } => f64::from(value),
                FirMatch::Float64 { value, .. } => value,
                _ => return None,
            };
            if *elem_type == FirType::Int32 {
                return None;
            }
            if *elem_type == FirType::Float32 {
                #[allow(clippy::cast_possible_truncation)]
                let x = real as f32;
                let x = if x.is_nan() { f32::NAN } else { x + 0.0 };
                let _ = write!(hex, "{:08x}", x.to_bits());
            } else {
                let x = if real.is_nan() { f64::NAN } else { real + 0.0 };
                let _ = write!(hex, "{:016x}", x.to_bits());
            }
        }
        self.helpers.insert(helper);
        Some(format!(
            "{}[{dtype}, {}](\"{hex}\")",
            helper.name(),
            values.len()
        ))
    }
}
