//! Native runtime descriptor derived from FIR for standalone Cranelift instances.
//!
//! This replaces the previous interpreter-sidecar dependency for UI/metadata
//! dispatch and basic instance initialization/reset behavior.

use std::collections::HashMap;

use fir::{AccessType, FirId, FirMatch, FirStore, FirType, match_fir};

/// Concrete initializer payload extracted from FIR globals/struct fields.
///
/// The descriptor keeps values in a Rust-native form so instance init/clear
/// can replay them directly into the backend `dsp*` state buffer.
#[derive(Clone, Debug)]
pub(crate) enum RuntimeFieldInit {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    I32Array(Vec<i32>),
    F32Array(Vec<f32>),
    F64Array(Vec<f64>),
}

/// UI declaration item replayed by the Cranelift instance wrapper.
///
/// This enum mirrors the FIR-side UI instructions closely enough for the FFI
/// runtime to rebuild `buildUserInterface` callbacks without reinterpreting FIR
/// at instance-construction time.
#[derive(Clone, Debug)]
pub(crate) enum RuntimeUiItem {
    OpenTabBox {
        label: String,
    },
    OpenHorizontalBox {
        label: String,
    },
    OpenVerticalBox {
        label: String,
    },
    CloseBox,
    Button {
        label: String,
        zone: String,
    },
    CheckButton {
        label: String,
        zone: String,
    },
    VerticalSlider {
        label: String,
        zone: String,
        init: f32,
        lo: f32,
        hi: f32,
        step: f32,
    },
    HorizontalSlider {
        label: String,
        zone: String,
        init: f32,
        lo: f32,
        hi: f32,
        step: f32,
    },
    NumEntry {
        label: String,
        zone: String,
        init: f32,
        lo: f32,
        hi: f32,
        step: f32,
    },
    HorizontalBargraph {
        label: String,
        zone: String,
        lo: f32,
        hi: f32,
    },
    VerticalBargraph {
        label: String,
        zone: String,
        lo: f32,
        hi: f32,
    },
    Soundfile {
        label: String,
        url: String,
        zone: String,
    },
    /// C++ parity: UI-scoped `declare(zone, key, value)` callback replay from
    /// FIR `AddMetaDeclare` inside `buildUserInterface`.
    Declare {
        zone: Option<String>,
        key: String,
        value: String,
    },
}

/// FIR-derived runtime metadata shared by Cranelift factories and instances.
///
/// The descriptor is computed once per compiled factory and then reused by each
/// instance for:
/// - constant/global initialization,
/// - clear/reset decisions,
/// - UI callback replay,
/// - metadata callback replay,
/// - sample-rate field updates.
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeDescriptor {
    pub(crate) field_inits: HashMap<String, RuntimeFieldInit>,
    /// The initial value of every control, at the precision the FIR carries
    /// (`f64`): a `-double` program must start from the very double its
    /// source wrote, not from its `f32` rounding.
    pub(crate) control_defaults: HashMap<String, f64>,
    /// Ordered `buildUserInterface` callback stream reconstructed from FIR.
    ///
    /// This is the Cranelift-side consumer of the grouped-UI rewrite: runtime
    /// instances no longer need to infer layout from signal widgets.
    pub(crate) ui_items: Vec<RuntimeUiItem>,
    /// Free-form `metadata()` callback entries only.
    ///
    /// UI-scoped `declare(zone, key, value)` belongs in [`Self::ui_items`], not
    /// here.
    pub(crate) meta_entries: Vec<(String, String)>,
    pub(crate) sample_rate_fields: Vec<String>,
}

/// Builds one native runtime descriptor from a FIR module.
///
/// The expected FIR source is the same module fed to the Cranelift backend
/// proper. This helper walks:
/// - `dsp_struct` and `globals` for field defaults and clear policy,
/// - `buildUserInterface` for UI declarations,
/// - `metadata` for free-form metadata key/value pairs.
pub(crate) fn build_runtime_descriptor(
    store: &FirStore,
    module: FirId,
) -> Result<RuntimeDescriptor, String> {
    let (dsp_struct, globals, functions) = match match_fir(store, module) {
        FirMatch::Module {
            dsp_struct,
            globals,
            functions,
            ..
        } => (dsp_struct, globals, functions),
        other => {
            return Err(format!(
                "expected FIR Module for runtime descriptor, got {other:?}"
            ));
        }
    };

    let mut desc = RuntimeDescriptor::default();
    for block in [dsp_struct, globals] {
        let items = match match_fir(store, block) {
            FirMatch::Block(items) => items,
            other => {
                return Err(format!(
                    "runtime descriptor expects FIR Block for module section, got {other:?}"
                ));
            }
        };
        for item in items {
            match match_fir(store, item) {
                FirMatch::DeclareVar {
                    name,
                    typ,
                    access: AccessType::Struct,
                    init,
                } => {
                    if let Some(value) = init.and_then(|id| decode_init_value(store, id, &typ)) {
                        desc.field_inits.insert(name.clone(), value);
                    }
                    if name == "fSampleRate" || name == "fSamplingFreq" {
                        desc.sample_rate_fields.push(name.clone());
                    }
                }
                FirMatch::DeclareTable {
                    name,
                    access: AccessType::Struct,
                    elem_type,
                    values,
                } => {
                    if let Some(init) = decode_table_values(store, &elem_type, &values) {
                        desc.field_inits.insert(name.clone(), init);
                    }
                }
                _ => {}
            }
        }
    }

    let function_items = match match_fir(store, functions) {
        FirMatch::Block(items) => items,
        other => {
            return Err(format!(
                "runtime descriptor expects functions block, got {other:?}"
            ));
        }
    };
    for fun in function_items {
        match match_fir(store, fun) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "buildUserInterface" => {
                collect_ui_items(store, body, &mut desc)?;
            }
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "metadata" => {
                collect_meta_items(store, body, &mut desc)?;
            }
            _ => {}
        }
    }

    Ok(desc)
}

/// Collects UI declaration items from FIR `buildUserInterface`.
///
/// This also records default control values for active widgets so
/// `instanceResetUserInterface` can restore them without re-running FIR.
///
/// Parity rule:
/// - FIR `AddMetaDeclare` inside `buildUserInterface` becomes
///   [`RuntimeUiItem::Declare`],
/// - it must not leak into the `metadata()` callback stream.
fn collect_ui_items(
    store: &FirStore,
    body: FirId,
    desc: &mut RuntimeDescriptor,
) -> Result<(), String> {
    let items = flatten_block(store, body)?;
    for item in items {
        match match_fir(store, item) {
            FirMatch::OpenBox { typ, label } => {
                desc.ui_items.push(match typ {
                    fir::UiBoxType::Tab => RuntimeUiItem::OpenTabBox { label },
                    fir::UiBoxType::Horizontal => RuntimeUiItem::OpenHorizontalBox { label },
                    fir::UiBoxType::Vertical => RuntimeUiItem::OpenVerticalBox { label },
                });
            }
            FirMatch::CloseBox => desc.ui_items.push(RuntimeUiItem::CloseBox),
            FirMatch::AddButton { typ, label, var } => {
                desc.ui_items.push(match typ {
                    fir::ButtonType::Button => RuntimeUiItem::Button {
                        label,
                        zone: var.clone(),
                    },
                    fir::ButtonType::Checkbox => RuntimeUiItem::CheckButton {
                        label,
                        zone: var.clone(),
                    },
                });
                desc.control_defaults.entry(var).or_insert(0.0);
            }
            FirMatch::AddSlider {
                typ,
                label,
                var,
                init,
                lo,
                hi,
                step,
            } => {
                desc.control_defaults.insert(var.clone(), init);
                let init = init as f32;
                desc.ui_items.push(match typ {
                    fir::SliderType::Horizontal => RuntimeUiItem::HorizontalSlider {
                        label,
                        zone: var,
                        init,
                        lo: lo as f32,
                        hi: hi as f32,
                        step: step as f32,
                    },
                    fir::SliderType::Vertical => RuntimeUiItem::VerticalSlider {
                        label,
                        zone: var,
                        init,
                        lo: lo as f32,
                        hi: hi as f32,
                        step: step as f32,
                    },
                    fir::SliderType::NumEntry => RuntimeUiItem::NumEntry {
                        label,
                        zone: var,
                        init,
                        lo: lo as f32,
                        hi: hi as f32,
                        step: step as f32,
                    },
                });
            }
            FirMatch::AddBargraph {
                typ,
                label,
                var,
                lo,
                hi,
            } => {
                desc.ui_items.push(match typ {
                    fir::BargraphType::Horizontal => RuntimeUiItem::HorizontalBargraph {
                        label,
                        zone: var,
                        lo: lo as f32,
                        hi: hi as f32,
                    },
                    fir::BargraphType::Vertical => RuntimeUiItem::VerticalBargraph {
                        label,
                        zone: var,
                        lo: lo as f32,
                        hi: hi as f32,
                    },
                });
            }
            FirMatch::AddSoundfile { label, url, var } => {
                desc.ui_items.push(RuntimeUiItem::Soundfile {
                    label,
                    url,
                    zone: var,
                });
            }
            FirMatch::AddMetaDeclare { var, key, value } => {
                desc.ui_items.push(RuntimeUiItem::Declare {
                    zone: (var != "0").then_some(var),
                    key,
                    value,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Collects metadata key/value declarations from FIR `metadata`.
fn collect_meta_items(
    store: &FirStore,
    body: FirId,
    desc: &mut RuntimeDescriptor,
) -> Result<(), String> {
    let items = flatten_block(store, body)?;
    for item in items {
        if let FirMatch::AddMetaDeclare { key, value, .. } = match_fir(store, item) {
            desc.meta_entries.push((key, value));
        }
    }
    Ok(())
}

/// Normalizes one FIR body to a flat statement vector.
///
/// Runtime descriptor extraction expects `buildUserInterface` and `metadata`
/// bodies to be plain FIR blocks.
fn flatten_block(store: &FirStore, body: FirId) -> Result<Vec<FirId>, String> {
    match match_fir(store, body) {
        FirMatch::Block(items) => Ok(items),
        other => Err(format!("expected FIR Block body, got {other:?}")),
    }
}

/// Decodes one FIR initializer into a native runtime field initializer.
///
/// This helper intentionally peels simple FIR `Cast(...)` wrappers so the
/// runtime descriptor records the target field value rather than the exact FIR
/// syntactic shape.
fn decode_init_value(store: &FirStore, id: FirId, typ: &FirType) -> Option<RuntimeFieldInit> {
    match (typ, match_fir(store, id)) {
        (_, FirMatch::Cast { value, .. }) => decode_init_value(store, value, typ),
        (FirType::Int32, FirMatch::Int32 { value, .. }) => Some(RuntimeFieldInit::I32(value)),
        (FirType::Int64, FirMatch::Int32 { value, .. }) => {
            Some(RuntimeFieldInit::I64(value as i64))
        }
        (FirType::Int64, FirMatch::Int64 { value, .. }) => Some(RuntimeFieldInit::I64(value)),
        (FirType::Float32 | FirType::FaustFloat, FirMatch::Float32 { value, .. }) => {
            Some(RuntimeFieldInit::F32(value))
        }
        (FirType::Float32 | FirType::FaustFloat, FirMatch::Float64 { value, .. }) => {
            Some(RuntimeFieldInit::F32(value as f32))
        }
        (FirType::Float64, FirMatch::Float64 { value, .. }) => Some(RuntimeFieldInit::F64(value)),
        (FirType::Float64, FirMatch::Float32 { value, .. }) => {
            Some(RuntimeFieldInit::F64(value as f64))
        }
        (FirType::Bool, FirMatch::Bool { value, .. }) => Some(RuntimeFieldInit::Bool(value)),
        (FirType::Array(inner, _), FirMatch::ValueArray { values, .. }) => {
            decode_array_values(store, inner, &values)
        }
        _ => None,
    }
}

/// Decodes FIR table initializers into one array-style runtime payload.
fn decode_table_values(
    store: &FirStore,
    elem_type: &FirType,
    values: &[FirId],
) -> Option<RuntimeFieldInit> {
    decode_array_values(store, elem_type, values)
}

/// Decodes FIR array literals for the subset of scalar element types used by
/// the current Cranelift runtime descriptor.
fn decode_array_values(
    store: &FirStore,
    elem_type: &FirType,
    values: &[FirId],
) -> Option<RuntimeFieldInit> {
    match elem_type {
        FirType::Int32 => values
            .iter()
            .map(|id| match match_fir(store, *id) {
                FirMatch::Int32 { value, .. } => Some(value),
                FirMatch::Bool { value, .. } => Some(i32::from(value)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(RuntimeFieldInit::I32Array),
        FirType::Float32 | FirType::FaustFloat => values
            .iter()
            .map(|id| match match_fir(store, *id) {
                FirMatch::Float32 { value, .. } => Some(value),
                FirMatch::Float64 { value, .. } => Some(value as f32),
                FirMatch::Int32 { value, .. } => Some(value as f32),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(RuntimeFieldInit::F32Array),
        FirType::Float64 => values
            .iter()
            .map(|id| match match_fir(store, *id) {
                FirMatch::Float64 { value, .. } => Some(value),
                FirMatch::Float32 { value, .. } => Some(value as f64),
                FirMatch::Int32 { value, .. } => Some(value as f64),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(RuntimeFieldInit::F64Array),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use compiler::{Compiler, SignalFirLane};

    use super::build_runtime_descriptor;

    #[test]
    fn runtime_descriptor_tracks_sample_rate_fields() {
        std::thread::Builder::new()
            .name("runtime-descriptor-test".to_owned())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let compiler = Compiler::new();
                // Keep this test self-contained instead of depending on
                // `stdfaust.lib` being installed on the CI runner.
                let source_name = "runtime_descriptor_sample_rate.dsp";
                let source = r#"
SR = fconstant(int fSamplingFreq, <math.h>);
process = min(SR, 192000.0) / 48000.0 : float;
"#;
                let fir = compiler
                    .compile_source_to_fir_with_lane(
                        source_name,
                        source,
                        SignalFirLane::TransformFastLane,
                    )
                    .expect("sample-rate source should lower to FIR");
                let runtime = build_runtime_descriptor(&fir.store, fir.module)
                    .expect("runtime descriptor builds");

                assert_eq!(runtime.sample_rate_fields, vec!["fSampleRate"]);
            })
            .expect("spawn runtime descriptor test")
            .join()
            .expect("runtime descriptor test should finish");
    }
}
