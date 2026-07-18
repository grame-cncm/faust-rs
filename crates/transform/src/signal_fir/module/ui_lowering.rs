//! UI control lowering, metadata emission, and `buildUserInterface` assembly.
//!
//! Defines [`UiLoweringState`], the sub-state struct that holds UI zone
//! registries, waveform table maps, and the `buildUserInterface` statement list.
//!
//! Manages the UI side of the Faust lifecycle:
//! - zone variable declaration for buttons, checkboxes, sliders, bargraphs,
//!   and soundfile controls;
//! - `addMetaDeclare` emission for per-control metadata key/value pairs;
//! - `emit_ui_program` — the recursive walk that emits the full
//!   `buildUserInterface(ui)` body from the [`UiProgram`] tree.
//!
//! All methods operate on the shared [`SignalToFirLower`] state and write
//! directly into `self.ui.ui_statements` for final assembly by `build_module`.

use super::*;

/// UI and table lowering accumulators.
#[derive(Default)]
pub(super) struct UiLoweringState {
    /// Maps each `ControlId` to its generated `FaustFloat` zone variable name.
    pub(super) ui_controls: HashMap<ControlId, String>,
    /// Maps each soundfile `ControlId` to its generated opaque zone variable name.
    pub(super) soundfiles: HashMap<ControlId, String>,
    /// Maps each waveform/table signal to its generated table variable name.
    pub(super) waveform_tables: HashMap<SigId, String>,
    /// Maps each waveform/table signal to its element count.
    pub(super) waveform_table_len: HashMap<SigId, usize>,
    /// Maps each waveform/table signal to the FIR storage class used for access.
    pub(super) table_access_by_sig: HashMap<SigId, AccessType>,
    /// `buildUserInterface` body: open/close box and add-control calls.
    pub(super) ui_statements: Vec<FirId>,
}

impl<'a> SignalToFirLower<'a> {
    /// Declares the `FaustFloat` struct zone variable for a button or checkbox, idempotent.
    pub(super) fn ensure_button_zone(
        &mut self,
        control: ControlId,
        typ: ButtonType,
    ) -> Result<String, SignalFirError> {
        if let Some(var) = self.ui.ui_controls.get(&control).cloned() {
            return Ok(var);
        }
        let spec = self.control_spec(control)?;
        let expected_kind = match typ {
            ButtonType::Button => ControlKind::Button,
            ButtonType::Checkbox => ControlKind::Checkbox,
        };
        if spec.kind != expected_kind {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "control id {control} kind mismatch: expected {expected_kind:?}, got {:?}",
                    spec.kind
                ),
            ));
        }
        let var = super::super::vector_ui::zone_name(expected_kind, control);
        let init = self.float_const(0.0);
        self.ensure_named_struct_var(&var, FirType::FaustFloat, Some(init));
        self.ui.ui_controls.insert(control, var.clone());
        Ok(var)
    }

    /// Lowers button/checkbox UI controls as zone-backed struct variables.
    pub(super) fn lower_button(
        &mut self,
        control: ControlId,
        typ: ButtonType,
    ) -> Result<FirId, SignalFirError> {
        let var = self.ensure_button_zone(control, typ)?;
        if self.ui.ui_controls.contains_key(&control) {
            // UI zone variable is FaustFloat (external); cast to real_ty for computation.
            let real_ty = self.real_ty();
            let mut b = FirBuilder::new(&mut self.store);
            let load = b.load_var(var, AccessType::Struct, FirType::FaustFloat);
            return Ok(b.cast(real_ty, load));
        }
        unreachable!("button zone should be inserted before loading")
    }

    /// Lowers slider-style UI controls and records metadata in
    /// `buildUserInterface`.
    pub(super) fn lower_slider(
        &mut self,
        control: ControlId,
        typ: SliderType,
    ) -> Result<FirId, SignalFirError> {
        let var = self.ensure_slider_zone(control, typ)?;
        if self.ui.ui_controls.contains_key(&control) {
            // UI zone variable is FaustFloat (external); cast to real_ty for computation.
            let real_ty = self.real_ty();
            let mut b = FirBuilder::new(&mut self.store);
            let load = b.load_var(var, AccessType::Struct, FirType::FaustFloat);
            return Ok(b.cast(real_ty, load));
        }
        unreachable!("slider zone should be inserted before loading")
    }

    /// Lowers bargraph UI nodes by creating UI descriptors and storing incoming
    /// runtime value in a dedicated control zone.
    pub(super) fn lower_bargraph(
        &mut self,
        control: ControlId,
        value: SigId,
        typ: BargraphType,
    ) -> Result<FirId, SignalFirError> {
        let _ = self.ensure_bargraph_zone(control, typ)?;
        // The incoming signal value is computed at internal real precision; cast
        // it to FaustFloat before writing to the external zone variable.
        let value = self.lower_signal(value)?;
        let var = self
            .ui
            .ui_controls
            .get(&control)
            .cloned()
            .expect("bargraph variable should exist after declaration");
        let mut b = FirBuilder::new(&mut self.store);
        let faust_value = b.cast(FirType::FaustFloat, value);
        self.regions
            .current_phases_mut()
            .immediate
            .push(b.store_var(var, AccessType::Struct, faust_value));
        Ok(value)
    }

    /// Lowers a soundfile declaration into UI-only registration and an opaque
    /// struct-backed runtime handle.
    pub(super) fn lower_soundfile(&mut self, control: ControlId) -> Result<FirId, SignalFirError> {
        let var = self.ensure_soundfile_zone(control)?;
        if self.ui.soundfiles.contains_key(&control) {
            let mut b = FirBuilder::new(&mut self.store);
            return Ok(b.load_var(var, AccessType::Struct, FirType::Sound));
        }
        unreachable!("soundfile zone should be inserted before loading")
    }

    /// Extracts the var name from a `SIGSOUNDFILE` signal node.
    pub(super) fn soundfile_var_from_signal(
        &mut self,
        sf: SigId,
    ) -> Result<String, SignalFirError> {
        match match_sig(self.arena, sf) {
            SigMatch::Soundfile(control) => self.ensure_soundfile_zone(control),
            _ => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "expected SIGSOUNDFILE node, got {}",
                    dump_sig_readable(self.arena, sf)
                ),
            )),
        }
    }

    /// Lowers `SIGSOUNDFILELENGTH(sf, part)` → `fSoundN->fLength[part]`.
    pub(super) fn lower_soundfile_length(
        &mut self,
        sf: SigId,
        part: SigId,
    ) -> Result<FirId, SignalFirError> {
        let var = self.soundfile_var_from_signal(sf)?;
        let part = self.lower_signal(part)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_soundfile_length(var, part))
    }

    /// Lowers `SIGSOUNDFILERATE(sf, part)` → `fSoundN->fSR[part]`.
    pub(super) fn lower_soundfile_rate(
        &mut self,
        sf: SigId,
        part: SigId,
    ) -> Result<FirId, SignalFirError> {
        let var = self.soundfile_var_from_signal(sf)?;
        let part = self.lower_signal(part)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_soundfile_rate(var, part))
    }

    /// Lowers `SIGSOUNDFILEBUFFER(sf, chan, part, ridx)` →
    /// `((FAUSTFLOAT**)fSoundN->fBuffers)[chan][fSoundN->fOffset[part] + ridx]`.
    pub(super) fn lower_soundfile_buffer(
        &mut self,
        node: SigId,
        sf: SigId,
        chan: SigId,
        part: SigId,
        ridx: SigId,
    ) -> Result<FirId, SignalFirError> {
        let var = self.soundfile_var_from_signal(sf)?;
        let chan = self.lower_signal(chan)?;
        let part = self.lower_signal(part)?;
        let idx = self.lower_signal(ridx)?;
        let typ = self.signal_fir_type(node)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_soundfile_buffer(var, chan, part, idx, typ))
    }

    /// Converts a label signal node to UTF-8 text fallback used by foreign refs.
    pub(super) fn label_text(&self, label: SigId) -> String {
        match self.arena.kind(label) {
            Some(NodeKind::Symbol(s)) => s.to_string(),
            Some(NodeKind::StringLiteral(s)) => s.to_string(),
            Some(NodeKind::Int(v)) => v.to_string(),
            Some(NodeKind::FloatBits(bits)) => f64::from_bits(*bits).to_string(),
            _ => "ui".to_owned(),
        }
    }

    /// Looks up the `ControlSpec` for `control`, returning an error if missing.
    pub(super) fn control_spec(
        &self,
        control: ControlId,
    ) -> Result<&ui::ControlSpec, SignalFirError> {
        self.ui_program.control(control).ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("missing UiProgram control spec for control id {control}"),
            )
        })
    }

    /// Returns the numeric range for `control`, returning an error if absent.
    ///
    /// `kind_name` is included in the error message for diagnostics only.
    pub(super) fn control_range(
        &self,
        control: ControlId,
        kind_name: &str,
    ) -> Result<ui::ControlRange, SignalFirError> {
        self.control_spec(control)?.range.ok_or_else(|| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("missing UI range for {kind_name} control id {control}"),
            )
        })
    }

    /// Declares the `FaustFloat` struct zone variable for a slider or numentry, idempotent.
    pub(super) fn ensure_slider_zone(
        &mut self,
        control: ControlId,
        typ: SliderType,
    ) -> Result<String, SignalFirError> {
        if let Some(var) = self.ui.ui_controls.get(&control).cloned() {
            return Ok(var);
        }
        let spec = self.control_spec(control)?;
        let expected_kind = match typ {
            SliderType::Horizontal => ControlKind::HSlider,
            SliderType::Vertical => ControlKind::VSlider,
            SliderType::NumEntry => ControlKind::NumEntry,
        };
        if spec.kind != expected_kind {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "control id {control} kind mismatch: expected {expected_kind:?}, got {:?}",
                    spec.kind
                ),
            ));
        }
        let var = super::super::vector_ui::zone_name(expected_kind, control);
        let range = self.control_range(
            control,
            match typ {
                SliderType::Horizontal => "hslider",
                SliderType::Vertical => "vslider",
                SliderType::NumEntry => "numentry",
            },
        )?;
        let init = self.float_const(range.init);
        self.ensure_named_struct_var(&var, FirType::FaustFloat, Some(init));
        self.ui.ui_controls.insert(control, var.clone());
        Ok(var)
    }

    /// Declares the `FaustFloat` struct zone variable for a bargraph, idempotent.
    pub(super) fn ensure_bargraph_zone(
        &mut self,
        control: ControlId,
        typ: BargraphType,
    ) -> Result<String, SignalFirError> {
        if let Some(var) = self.ui.ui_controls.get(&control).cloned() {
            return Ok(var);
        }
        let spec = self.control_spec(control)?;
        let expected_kind = match typ {
            BargraphType::Horizontal => ControlKind::HBargraph,
            BargraphType::Vertical => ControlKind::VBargraph,
        };
        if spec.kind != expected_kind {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "control id {control} kind mismatch: expected {expected_kind:?}, got {:?}",
                    spec.kind
                ),
            ));
        }
        let var = super::super::vector_ui::zone_name(expected_kind, control);
        let init = self.float_const(0.0);
        self.ensure_named_struct_var(&var, FirType::FaustFloat, Some(init));
        self.ui.ui_controls.insert(control, var.clone());
        Ok(var)
    }

    /// Declares the opaque `Sound` struct zone variable for a soundfile, idempotent.
    pub(super) fn ensure_soundfile_zone(
        &mut self,
        control: ControlId,
    ) -> Result<String, SignalFirError> {
        if let Some(var) = self.ui.soundfiles.get(&control).cloned() {
            return Ok(var);
        }
        let spec = self.control_spec(control)?;
        if spec.kind != ControlKind::Soundfile {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "control id {control} kind mismatch: expected {:?}, got {:?}",
                    ControlKind::Soundfile,
                    spec.kind
                ),
            ));
        }
        let var = super::super::vector_ui::zone_name(ControlKind::Soundfile, control);
        self.ensure_named_struct_var(&var, FirType::Sound, None);
        self.ui.soundfiles.insert(control, var.clone());
        Ok(var)
    }

    /// Drives emission of the entire `buildUserInterface` body from the root UI node.
    ///
    /// Clears any previous `ui_statements` accumulator before walking the tree.
    pub(super) fn emit_ui_program(&mut self) -> Result<(), SignalFirError> {
        if self.ui_program.is_empty() {
            self.ui.ui_statements.clear();
            return Ok(());
        }
        let mut zones = std::collections::BTreeMap::new();
        let controls = self
            .ui_program
            .controls
            .iter()
            .map(|spec| (spec.id, spec.kind))
            .collect::<Vec<_>>();
        for (control, kind) in controls {
            match kind {
                ControlKind::Button => {
                    self.ensure_button_zone(control, ButtonType::Button)?;
                }
                ControlKind::Checkbox => {
                    self.ensure_button_zone(control, ButtonType::Checkbox)?;
                }
                ControlKind::VSlider => {
                    self.ensure_slider_zone(control, SliderType::Vertical)?;
                }
                ControlKind::HSlider => {
                    self.ensure_slider_zone(control, SliderType::Horizontal)?;
                }
                ControlKind::NumEntry => {
                    self.ensure_slider_zone(control, SliderType::NumEntry)?;
                }
                ControlKind::VBargraph => {
                    self.ensure_bargraph_zone(control, BargraphType::Vertical)?;
                }
                ControlKind::HBargraph => {
                    self.ensure_bargraph_zone(control, BargraphType::Horizontal)?;
                }
                ControlKind::Soundfile => {
                    self.ensure_soundfile_zone(control)?;
                }
            }
            let zone = super::super::vector_ui::control_zone(self.ui_program, control).map_err(
                |detail| SignalFirError::new(SignalFirErrorCode::UnsupportedSignalNode, detail),
            )?;
            zones.insert(control, zone);
        }
        self.ui.ui_statements =
            super::super::vector_ui::build_ui_statements(self.ui_program, &zones, &mut self.store)
                .map_err(|detail| {
                    SignalFirError::new(SignalFirErrorCode::UnsupportedSignalNode, detail)
                })?;
        Ok(())
    }
}
