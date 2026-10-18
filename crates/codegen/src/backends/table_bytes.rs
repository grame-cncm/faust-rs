//! The bytes of a static table's initializer, for the backends that lay
//! tables out in memory themselves (cranelift JIT data, wasm data segments).
//!
//! The two encoders had drifted: cranelift skipped a value of the wrong kind
//! without a word, wasm refused a `Float32` value in a `Float64` table, and
//! wasm wrote a `Float32` value of a `FaustFloat` table as 4 bytes under
//! `-double`, where every read uses an 8-byte stride. One rule now: the
//! element type fixes the width (`FaustFloat` follows the precision), a real
//! value converts to it, and anything else is an error.

use fir::{FirId, FirMatch, FirStore, FirType, match_fir};

/// Appends the little-endian bytes of one table element.
///
/// `Bool` and `Int32` elements take 4 bytes; a real element takes 4 or 8
/// bytes as its type (and, for `FaustFloat`, `double_precision`) says.
///
/// # Errors
/// `elem_type=…, value=…` when the value is not a literal of a kind the
/// element type holds; the caller says which table and backend.
pub(crate) fn append_element(
    store: &FirStore,
    value: FirId,
    elem_type: &FirType,
    double_precision: bool,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let wide = match elem_type {
        FirType::Float64 => Some(true),
        FirType::Float32 => Some(false),
        FirType::FaustFloat => Some(double_precision),
        _ => None,
    };
    match (elem_type, wide, match_fir(store, value)) {
        (FirType::Bool, _, FirMatch::Bool { value, .. }) => {
            out.extend_from_slice(&u32::from(value).to_le_bytes());
        }
        (FirType::Int32, _, FirMatch::Int32 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (_, Some(true), FirMatch::Float64 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (_, Some(true), FirMatch::Float32 { value, .. }) => {
            out.extend_from_slice(&f64::from(value).to_le_bytes());
        }
        (_, Some(false), FirMatch::Float32 { value, .. }) => {
            out.extend_from_slice(&value.to_le_bytes());
        }
        (_, Some(false), FirMatch::Float64 { value, .. }) => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a single-precision table stores its values as f32"
            )]
            let narrowed = value as f32;
            out.extend_from_slice(&narrowed.to_le_bytes());
        }
        (_, _, other) => return Err(format!("elem_type={elem_type:?}, value={other:?}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fir::FirBuilder;

    fn bytes(
        elem_type: &FirType,
        double: bool,
        build: impl FnOnce(&mut FirBuilder<'_>) -> FirId,
    ) -> Result<Vec<u8>, String> {
        let mut store = FirStore::new();
        let value = build(&mut FirBuilder::new(&mut store));
        let mut out = Vec::new();
        append_element(&store, value, elem_type, double, &mut out).map(|()| out)
    }

    #[test]
    fn the_element_type_fixes_the_width() {
        let half = |b: &mut FirBuilder<'_>| b.float32(0.5);
        assert_eq!(
            bytes(&FirType::Float32, false, half).unwrap(),
            0.5_f32.to_le_bytes()
        );
        assert_eq!(
            bytes(&FirType::Float64, false, half).unwrap(),
            0.5_f64.to_le_bytes()
        );
        // `FaustFloat` follows the precision: 8 bytes under `-double`, the
        // case wasm wrote as 4.
        assert_eq!(
            bytes(&FirType::FaustFloat, true, half).unwrap(),
            0.5_f64.to_le_bytes()
        );
        assert_eq!(
            bytes(&FirType::FaustFloat, false, half).unwrap(),
            0.5_f32.to_le_bytes()
        );
        let wide = |b: &mut FirBuilder<'_>| b.float64(0.25);
        assert_eq!(
            bytes(&FirType::Float32, false, wide).unwrap(),
            0.25_f32.to_le_bytes()
        );
    }

    #[test]
    fn integers_and_booleans_take_four_bytes() {
        assert_eq!(
            bytes(&FirType::Int32, false, |b| b.int32(-2)).unwrap(),
            (-2_i32).to_le_bytes()
        );
        assert_eq!(
            bytes(&FirType::Bool, false, |b| b.bool_(true)).unwrap(),
            1_u32.to_le_bytes()
        );
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_an_error_not_skipped() {
        assert!(bytes(&FirType::Int32, false, |b| b.float32(1.0)).is_err());
        assert!(bytes(&FirType::Float32, false, |b| b.int32(1)).is_err());
    }
}
