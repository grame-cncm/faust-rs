//! Shared validation helpers for canonical Faust DSP API FIR signatures.

use fir::{FirType, NamedType};

/// Returns `true` when `typ` matches the canonical Faust `compute` FIR signature:
/// `void compute(dsp, int, FAUSTFLOAT**, FAUSTFLOAT**)`.
pub(crate) fn is_canonical_compute_signature(typ: &FirType) -> bool {
    let FirType::Fun { args, .. } = typ else {
        return false;
    };
    matches!(
        args.as_slice(),
        [
            FirType::Ptr(dsp),
            FirType::Int32,
            FirType::Ptr(inner_inputs),
            FirType::Ptr(inner_outputs)
        ] if matches!(dsp.as_ref(), FirType::Obj)
            && matches!(
            (inner_inputs.as_ref(), inner_outputs.as_ref()),
            (
                FirType::Ptr(ff_inputs),
                FirType::Ptr(ff_outputs)
            ) if matches!(
                (ff_inputs.as_ref(), ff_outputs.as_ref()),
                (FirType::FaustFloat, FirType::FaustFloat)
            )
        )
    )
}

/// Validates one canonical DSP API function signature when `name` matches a
/// reserved Faust method (`metadata`, `instanceConstants`, etc.).
///
/// Returns `Ok(())` for non-reserved function names.
pub(crate) fn validate_canonical_dsp_api_signature(
    name: &str,
    typ: &FirType,
    named_args: &[NamedType],
) -> Result<(), String> {
    let Some((expected_args, expected_ret, api_sig)) = expected_signature(name) else {
        return Ok(());
    };

    let FirType::Fun { args, ret } = typ else {
        return Err(format!(
            "invalid FIR signature for {name}: expected {api_sig}, got non-function type {typ:?}"
        ));
    };

    if *args != expected_args || ret.as_ref() != &expected_ret {
        return Err(format!(
            "invalid FIR signature for {name}: expected {api_sig}, got {typ:?}"
        ));
    }

    if named_args.len() != expected_args.len()
        || named_args
            .iter()
            .zip(expected_args.iter())
            .any(|(named, expected)| named.typ != *expected)
    {
        return Err(format!(
            "invalid FIR named args for {name}: expected types {expected_args:?}, got {named_args:?}"
        ));
    }

    Ok(())
}

/// Returns the canonical DSP API signature expected for a reserved method name.
///
/// The returned tuple is `(arg_types, return_type, human_readable_signature)`.
fn expected_signature(name: &str) -> Option<(Vec<FirType>, FirType, &'static str)> {
    match name {
        "metadata" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Meta],
            FirType::Void,
            "void metadata(dsp, Meta*)",
        )),
        "instanceConstants" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::Int32],
            FirType::Void,
            "void instanceConstants(dsp, int)",
        )),
        "instanceResetUserInterface" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj))],
            FirType::Void,
            "void instanceResetUserInterface(dsp)",
        )),
        "instanceClear" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj))],
            FirType::Void,
            "void instanceClear(dsp)",
        )),
        "buildUserInterface" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj)), FirType::UI],
            FirType::Void,
            "void buildUserInterface(dsp, UI*)",
        )),
        "compute" => Some((
            vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Int32,
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
                FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat)))),
            ],
            FirType::Void,
            "void compute(dsp, int, FAUSTFLOAT**, FAUSTFLOAT**)",
        )),
        // Execution-options port plan §4.5: optional execution entry points.
        // `control` performs the externally scheduled control-rate work;
        // `frame` processes exactly one sample over flat channel arrays —
        // no block count, no per-channel pointer table.
        "control" => Some((
            vec![FirType::Ptr(Box::new(FirType::Obj))],
            FirType::Void,
            "void control(dsp)",
        )),
        "frame" => Some((
            vec![
                FirType::Ptr(Box::new(FirType::Obj)),
                FirType::Ptr(Box::new(FirType::FaustFloat)),
                FirType::Ptr(Box::new(FirType::FaustFloat)),
            ],
            FirType::Void,
            "void frame(dsp, FAUSTFLOAT*, FAUSTFLOAT*)",
        )),
        _ => None,
    }
}
