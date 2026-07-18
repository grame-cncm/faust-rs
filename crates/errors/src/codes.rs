//! Stable diagnostic code taxonomy.
//!
//! Codes are organized by phase family and intended to be stable across
//! parser/eval/propagate/compiler integrations and test snapshots.

use crate::DiagnosticCode;

/// Source reader I/O failure.
pub const SRC_IO_ERROR: DiagnosticCode = DiagnosticCode("FRS-SRC-0001");
/// Imported file could not be resolved.
pub const SRC_UNRESOLVED_IMPORT: DiagnosticCode = DiagnosticCode("FRS-SRC-0002");
/// Import graph contains a cycle.
pub const SRC_IMPORT_CYCLE: DiagnosticCode = DiagnosticCode("FRS-SRC-0003");

/// Lexer encountered an invalid token sequence.
pub const LEX_INVALID_TOKEN: DiagnosticCode = DiagnosticCode("FRS-LEX-0001");

/// Parser encountered an unexpected token.
pub const PARSE_UNEXPECTED_TOKEN: DiagnosticCode = DiagnosticCode("FRS-PARSE-0001");
/// Parser recovered from an error and emitted recovery diagnostics.
pub const PARSE_RECOVERY: DiagnosticCode = DiagnosticCode("FRS-PARSE-0002");
/// Parser encountered an invalid literal form.
pub const PARSE_INVALID_LITERAL: DiagnosticCode = DiagnosticCode("FRS-PARSE-0003");

/// `process` definition is missing.
pub const EVAL_MISSING_PROCESS: DiagnosticCode = DiagnosticCode("FRS-EVAL-0001");
/// Symbol lookup failed during eval.
pub const EVAL_UNDEFINED_SYMBOL: DiagnosticCode = DiagnosticCode("FRS-EVAL-0002");
/// Arity mismatch detected during eval.
pub const EVAL_ARITY_MISMATCH: DiagnosticCode = DiagnosticCode("FRS-EVAL-0003");
/// Invalid iteration construct detected during eval.
pub const EVAL_ITERATION_INVALID: DiagnosticCode = DiagnosticCode("FRS-EVAL-0004");
/// Symbol redefined with a different value in the same lexical scope.
///
/// C++ equivalent: the `addLayerDef` check in `environment.cpp` that calls
/// `throw faustexception("redefinition of symbols are not allowed: ...")`.
pub const EVAL_REDEFINED_SYMBOL: DiagnosticCode = DiagnosticCode("FRS-EVAL-0005");
/// Slider/numentry init value is outside the [min, max] range.
///
/// C++ equivalent: the `checkRange` check in `eval.cpp` that emits
/// `"init = ... outside of [min max] range in '...'"`.
pub const EVAL_SLIDER_INIT_OUT_OF_RANGE: DiagnosticCode = DiagnosticCode("FRS-EVAL-0006");
/// Generic eval failure fallback code.
pub const EVAL_GENERIC_FAILURE: DiagnosticCode = DiagnosticCode("FRS-EVAL-0099");

/// Unsupported box node in propagate.
pub const PROP_UNSUPPORTED_BOX: DiagnosticCode = DiagnosticCode("FRS-PROP-0001");
/// Arity mismatch in propagate composition rules.
pub const PROP_ARITY_MISMATCH: DiagnosticCode = DiagnosticCode("FRS-PROP-0002");
/// Recursion/projection contract mismatch in propagate.
pub const PROP_RECURSION_MISMATCH: DiagnosticCode = DiagnosticCode("FRS-PROP-0003");
/// Automatic differentiation reached a clock-domain boundary it cannot cross.
pub const PROP_AD_CLOCK_BOUNDARY: DiagnosticCode = DiagnosticCode("FRS-PROP-0004");
/// Generic propagate failure fallback code.
pub const PROP_GENERIC_FAILURE: DiagnosticCode = DiagnosticCode("FRS-PROP-0099");

/// Invalid options passed to signal->FIR lowering.
pub const SFIR_INVALID_OPTIONS: DiagnosticCode = DiagnosticCode("FRS-SFIR-0001");
/// Empty signal list provided to signal->FIR lowering.
pub const SFIR_EMPTY_SIGNAL_LIST: DiagnosticCode = DiagnosticCode("FRS-SFIR-0002");
/// Signal outputs arity mismatch in signal->FIR lowering.
pub const SFIR_OUTPUT_ARITY_MISMATCH: DiagnosticCode = DiagnosticCode("FRS-SFIR-0003");
/// Unsupported signal node in signal->FIR lowering.
pub const SFIR_UNSUPPORTED_SIGNAL_NODE: DiagnosticCode = DiagnosticCode("FRS-SFIR-0004");
/// Unsupported binary operator in signal->FIR lowering.
pub const SFIR_UNSUPPORTED_BINOP: DiagnosticCode = DiagnosticCode("FRS-SFIR-0005");
/// Input index out of range in signal->FIR lowering.
pub const SFIR_INPUT_INDEX_OUT_OF_RANGE: DiagnosticCode = DiagnosticCode("FRS-SFIR-0006");
/// Clocked node (`ondemand`/`upsampling`/`downsampling` machinery) reached
/// signal->FIR lowering before the clock-domain back half is ported.
pub const SFIR_CLOCKED_NOT_LOWERED: DiagnosticCode = DiagnosticCode("FRS-SFIR-0007");
/// Clock-environment inference / hierarchical-graph validation failed.
pub const SFIR_CLOCK_ANALYSIS: DiagnosticCode = DiagnosticCode("FRS-SFIR-0008");

/// FIR verifier error diagnostic (details in notes: `fir_code=...`).
pub const FIR_VERIFY_ERROR: DiagnosticCode = DiagnosticCode("FRS-FIR-0001");
/// FIR verifier warning diagnostic (details in notes: `fir_code=...`).
pub const FIR_VERIFY_WARNING: DiagnosticCode = DiagnosticCode("FRS-FIR-0002");

/// Parse stage failed in top-level compiler pipeline.
pub const COMP_PARSE_FAILED: DiagnosticCode = DiagnosticCode("FRS-COMP-0001");
/// Eval stage failed in top-level compiler pipeline.
pub const COMP_EVAL_FAILED: DiagnosticCode = DiagnosticCode("FRS-COMP-0002");
/// Propagate stage failed in top-level compiler pipeline.
pub const COMP_PROPAGATE_FAILED: DiagnosticCode = DiagnosticCode("FRS-COMP-0003");
/// Signal type validation failed in top-level compiler pipeline.
pub const COMP_TYPE_FAILED: DiagnosticCode = DiagnosticCode("FRS-COMP-0004");

/// Returns all built-in stable diagnostic codes.
#[must_use]
/// Returns the full stable registry of known diagnostic codes.
pub fn all_codes() -> &'static [DiagnosticCode] {
    &[
        SRC_IO_ERROR,
        SRC_UNRESOLVED_IMPORT,
        SRC_IMPORT_CYCLE,
        LEX_INVALID_TOKEN,
        PARSE_UNEXPECTED_TOKEN,
        PARSE_RECOVERY,
        PARSE_INVALID_LITERAL,
        EVAL_MISSING_PROCESS,
        EVAL_UNDEFINED_SYMBOL,
        EVAL_ARITY_MISMATCH,
        EVAL_ITERATION_INVALID,
        EVAL_REDEFINED_SYMBOL,
        EVAL_GENERIC_FAILURE,
        PROP_UNSUPPORTED_BOX,
        PROP_ARITY_MISMATCH,
        PROP_RECURSION_MISMATCH,
        PROP_AD_CLOCK_BOUNDARY,
        PROP_GENERIC_FAILURE,
        SFIR_INVALID_OPTIONS,
        SFIR_EMPTY_SIGNAL_LIST,
        SFIR_OUTPUT_ARITY_MISMATCH,
        SFIR_UNSUPPORTED_SIGNAL_NODE,
        SFIR_UNSUPPORTED_BINOP,
        SFIR_INPUT_INDEX_OUT_OF_RANGE,
        SFIR_CLOCKED_NOT_LOWERED,
        SFIR_CLOCK_ANALYSIS,
        FIR_VERIFY_ERROR,
        FIR_VERIFY_WARNING,
        COMP_PARSE_FAILED,
        COMP_EVAL_FAILED,
        COMP_PROPAGATE_FAILED,
        COMP_TYPE_FAILED,
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::all_codes;

    fn is_valid_code(raw: &str) -> bool {
        let mut parts = raw.split('-');
        let Some(prefix) = parts.next() else {
            return false;
        };
        let Some(family) = parts.next() else {
            return false;
        };
        let Some(num) = parts.next() else {
            return false;
        };
        if parts.next().is_some() {
            return false;
        }
        prefix == "FRS"
            && family
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            && num.len() == 4
            && num.bytes().all(|b| b.is_ascii_digit())
    }

    #[test]
    fn all_codes_follow_stable_format() {
        for code in all_codes() {
            assert!(
                is_valid_code(code.0),
                "invalid diagnostic code format: {}",
                code.0
            );
        }
    }

    #[test]
    fn all_codes_are_unique() {
        let mut seen = HashSet::new();
        for code in all_codes() {
            assert!(seen.insert(code.0), "duplicate diagnostic code: {}", code.0);
        }
    }
}
