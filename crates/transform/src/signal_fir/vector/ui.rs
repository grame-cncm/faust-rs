//! Shared checked UI lifecycle materialization for scalar and vector modules.
//!
//! # C++ provenance and adaptation
//! This mirrors the zone declarations, `instanceResetUserInterface`, and
//! grouped `buildUserInterface` calls assembled ultimately by C++
//! `CodeContainer`. Both Rust FIR paths use the same zone naming and grouped
//! UI statement builder; the vector path additionally materializes an explicit
//! artifact so final module verification can require exact declaration,
//! reset, and UI statement coverage.

use std::collections::BTreeMap;

use fir::{
    AccessType, BargraphType, ButtonType, FirBuilder, FirId, FirStore, FirType, SliderRange,
    SliderType, UiBoxType,
};
use ui::{ControlId, ControlKind, UiGroupKind, UiMatch, UiProgram, match_ui};

/// One checked UI zone available to vector signal lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VectorUiZone {
    pub control: ControlId,
    pub kind: ControlKind,
    pub name: String,
}

/// FIR statements required by the Faust UI lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VectorUiFir {
    pub zones: BTreeMap<ControlId, VectorUiZone>,
    pub struct_declarations: Vec<FirId>,
    pub reset_statements: Vec<FirId>,
    pub build_statements: Vec<FirId>,
}

/// Stable zone name shared by vector UI lifecycle and signal lowering.
pub(crate) fn zone_name(kind: ControlKind, control: ControlId) -> String {
    let prefix = match kind {
        ControlKind::Button => "fButton",
        ControlKind::Checkbox => "fCheckbox",
        ControlKind::VSlider => "fVslider",
        ControlKind::HSlider => "fHslider",
        ControlKind::NumEntry => "fEntry",
        ControlKind::VBargraph => "fVbargraph",
        ControlKind::HBargraph => "fHbargraph",
        ControlKind::Soundfile => "fSound",
    };
    format!("{prefix}{control}")
}

/// Looks up and validates one dense `UiProgram` control entry.
pub(crate) fn control_zone(
    program: &UiProgram,
    control: ControlId,
) -> Result<VectorUiZone, String> {
    let spec = program
        .control(control)
        .ok_or_else(|| format!("missing UiProgram control spec for control id {control}"))?;
    if spec.id != control {
        return Err(format!(
            "UiProgram control slot {control} contains mismatched id {}",
            spec.id
        ));
    }
    Ok(VectorUiZone {
        control,
        kind: spec.kind,
        name: zone_name(spec.kind, control),
    })
}

/// Builds the complete grouped UI lifecycle artifact in `store`.
pub(crate) fn build_vector_ui_fir(
    program: &UiProgram,
    real_type: &FirType,
    store: &mut FirStore,
) -> Result<VectorUiFir, String> {
    let mut zones = BTreeMap::new();
    let mut struct_declarations = Vec::new();
    let mut reset_statements = Vec::new();

    for spec in &program.controls {
        let zone = control_zone(program, spec.id)?;
        let typ = if spec.kind == ControlKind::Soundfile {
            FirType::Sound
        } else {
            FirType::FaustFloat
        };
        struct_declarations.push(FirBuilder::new(store).declare_var(
            zone.name.clone(),
            typ,
            AccessType::Struct,
            None,
        ));
        if spec.kind != ControlKind::Soundfile {
            let initial = match spec.kind {
                ControlKind::VSlider | ControlKind::HSlider | ControlKind::NumEntry => {
                    spec.range
                        .ok_or_else(|| format!("control {} has no slider range", spec.id))?
                        .init
                }
                _ => 0.0,
            };
            let initial = match real_type {
                FirType::Float32 => FirBuilder::new(store).float32(initial as f32),
                FirType::Float64 => FirBuilder::new(store).float64(initial),
                other => return Err(format!("invalid vector UI real type {other:?}")),
            };
            reset_statements.push(FirBuilder::new(store).store_var(
                zone.name.clone(),
                AccessType::Struct,
                initial,
            ));
        }
        if zones.insert(spec.id, zone).is_some() {
            return Err(format!("duplicate UiProgram control id {}", spec.id));
        }
    }

    let build_statements = build_ui_statements(program, &zones, store)?;
    Ok(VectorUiFir {
        zones,
        struct_declarations,
        reset_statements,
        build_statements,
    })
}

/// Builds the canonical grouped `buildUserInterface` statement sequence used
/// by both scalar and checked-vector FIR assembly.
pub(crate) fn build_ui_statements(
    program: &UiProgram,
    zones: &BTreeMap<ControlId, VectorUiZone>,
    store: &mut FirStore,
) -> Result<Vec<FirId>, String> {
    let mut statements = Vec::new();
    if program.emit_ui {
        emit_ui_node(program, program.root, zones, store, &mut statements)?;
    }
    Ok(statements)
}

fn emit_metadata(
    store: &mut FirStore,
    out: &mut Vec<FirId>,
    target: &str,
    entries: &[(String, String)],
) {
    for (key, value) in entries {
        out.push(FirBuilder::new(store).add_meta_declare(
            target.to_owned(),
            key.clone(),
            value.clone(),
        ));
    }
}

fn emit_ui_node(
    program: &UiProgram,
    node: ui::UiId,
    zones: &BTreeMap<ControlId, VectorUiZone>,
    store: &mut FirStore,
    out: &mut Vec<FirId>,
) -> Result<(), String> {
    match match_ui(&program.arena, node) {
        UiMatch::Group {
            kind,
            label,
            metadata,
            children,
        } => {
            emit_metadata(store, out, "0", &metadata);
            let typ = match kind {
                UiGroupKind::Vertical => UiBoxType::Vertical,
                UiGroupKind::Horizontal => UiBoxType::Horizontal,
                UiGroupKind::Tab => UiBoxType::Tab,
            };
            out.push(FirBuilder::new(store).open_box(typ, label));
            for child in children {
                emit_ui_node(program, child, zones, store, out)?;
            }
            out.push(FirBuilder::new(store).close_box());
            Ok(())
        }
        UiMatch::InputControl(control) | UiMatch::OutputControl(control) => {
            let spec = program.control(control).ok_or_else(|| {
                format!("missing UiProgram control spec for control id {control}")
            })?;
            let zone = zones
                .get(&control)
                .ok_or_else(|| format!("missing vector UI zone for control id {control}"))?;
            emit_metadata(store, out, &zone.name, &spec.metadata);
            let statement = match spec.kind {
                ControlKind::Button | ControlKind::Checkbox => FirBuilder::new(store).add_button(
                    if spec.kind == ControlKind::Button {
                        ButtonType::Button
                    } else {
                        ButtonType::Checkbox
                    },
                    spec.label.clone(),
                    zone.name.clone(),
                ),
                ControlKind::VSlider | ControlKind::HSlider | ControlKind::NumEntry => {
                    let range = spec
                        .range
                        .ok_or_else(|| format!("control {control} has no slider range"))?;
                    let typ = match spec.kind {
                        ControlKind::VSlider => SliderType::Vertical,
                        ControlKind::HSlider => SliderType::Horizontal,
                        ControlKind::NumEntry => SliderType::NumEntry,
                        _ => unreachable!(),
                    };
                    FirBuilder::new(store).add_slider(
                        typ,
                        spec.label.clone(),
                        zone.name.clone(),
                        SliderRange {
                            init: range.init,
                            lo: range.min,
                            hi: range.max,
                            step: range.step,
                        },
                    )
                }
                ControlKind::VBargraph | ControlKind::HBargraph => {
                    let range = spec
                        .range
                        .ok_or_else(|| format!("control {control} has no bargraph range"))?;
                    FirBuilder::new(store).add_bargraph(
                        if spec.kind == ControlKind::VBargraph {
                            BargraphType::Vertical
                        } else {
                            BargraphType::Horizontal
                        },
                        spec.label.clone(),
                        zone.name.clone(),
                        range.min,
                        range.max,
                    )
                }
                ControlKind::Soundfile => {
                    return Err(format!(
                        "soundfile control {control} appears as a numeric UI leaf"
                    ));
                }
            };
            out.push(statement);
            Ok(())
        }
        UiMatch::Soundfile(control) => {
            let spec = program
                .control(control)
                .ok_or_else(|| format!("missing UiProgram soundfile control {control}"))?;
            let zone = zones
                .get(&control)
                .ok_or_else(|| format!("missing vector soundfile zone {control}"))?;
            let url = spec
                .metadata
                .iter()
                .find_map(|(key, value)| (key == "url").then(|| value.clone()))
                .unwrap_or_default();
            out.push(FirBuilder::new(store).add_soundfile_with_url(
                spec.label.clone(),
                url,
                zone.name.clone(),
            ));
            Ok(())
        }
        UiMatch::Unknown => Err("malformed UiProgram node".to_owned()),
    }
}
