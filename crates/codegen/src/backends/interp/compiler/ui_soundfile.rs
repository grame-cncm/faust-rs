//! `ui_soundfile` half of the FBC compiler.
//!
//! UI widget declarations, metadata, and soundfile slots.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
    /// # Source provenance (C++)
    /// - `visit(OpenboxInst*)`.
    pub(super) fn compile_open_box(
        &mut self,
        typ: &UiBoxType,
        label: &str,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            UiBoxType::Vertical => FbcOpcode::OpenVerticalBox,
            UiBoxType::Horizontal => FbcOpcode::OpenHorizontalBox,
            UiBoxType::Tab => FbcOpcode::OpenTabBox,
        };
        self.ui_instructions
            .push(FbcUiInstruction::open_box(opcode, label));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(CloseboxInst*)`.
    pub(super) fn compile_close_box(&mut self) -> Result<(), CompileError> {
        self.ui_instructions
            .push(FbcUiInstruction::new(FbcOpcode::CloseBox));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddButtonInst*)`.
    pub(super) fn compile_add_button(
        &mut self,
        typ: &ButtonType,
        label: &str,
        var: &str,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            ButtonType::Button => FbcOpcode::AddButton,
            ButtonType::Checkbox => FbcOpcode::AddCheckButton,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::widget(
            opcode,
            offset,
            label,
            R::default(),
            R::default(),
            R::default(),
            R::default(),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddSliderInst*)`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_add_slider(
        &mut self,
        typ: &SliderType,
        label: &str,
        var: &str,
        init: f64,
        lo: f64,
        hi: f64,
        step: f64,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            SliderType::Horizontal => FbcOpcode::AddHorizontalSlider,
            SliderType::Vertical => FbcOpcode::AddVerticalSlider,
            SliderType::NumEntry => FbcOpcode::AddNumEntry,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::widget(
            opcode,
            offset,
            label,
            R::from_f64(init),
            R::from_f64(lo),
            R::from_f64(hi),
            R::from_f64(step),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddBargraphInst*)`.
    pub(super) fn compile_add_bargraph(
        &mut self,
        typ: &BargraphType,
        label: &str,
        var: &str,
        lo: f64,
        hi: f64,
    ) -> Result<(), CompileError> {
        let opcode = match typ {
            BargraphType::Horizontal => FbcOpcode::AddHorizontalBargraph,
            BargraphType::Vertical => FbcOpcode::AddVerticalBargraph,
        };
        let offset = self.get_field_offset(var);
        self.ui_instructions.push(FbcUiInstruction::bargraph(
            opcode,
            offset,
            label,
            R::from_f64(lo),
            R::from_f64(hi),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddSoundfileInst*)`.
    pub(super) fn compile_add_soundfile(
        &mut self,
        label: &str,
        url: &str,
        var: &str,
    ) -> Result<(), CompileError> {
        // Register (or look up) this variable's soundfile slot.
        let slot = self.alloc_soundfile_slot(var);
        let mut instr = FbcUiInstruction::new(FbcOpcode::AddSoundfile);
        instr.label = label.to_string();
        // Store URL in `key` field — mirrors how `dispatch_ui_*` passes it to the callback.
        instr.key = url.to_string();
        // Store slot index in `offset` so instances can populate the right soundfile slot.
        instr.offset = slot as i32;
        self.ui_instructions.push(instr);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Soundfile access
    // -----------------------------------------------------------------------

    /// Allocates (or reuses) a soundfile slot index for `name`.
    ///
    /// Soundfile variables (`fSoundN`) are tracked in a separate slot table
    /// rather than in the int/real heap, because they are runtime object
    /// references — not scalar values.
    pub(super) fn alloc_soundfile_slot(&mut self, name: &str) -> usize {
        if let Some(&slot) = self.soundfile_slots.get(name) {
            return slot;
        }
        let slot = self.num_soundfile_slots;
        self.num_soundfile_slots += 1;
        self.soundfile_slots.insert(name.to_string(), slot);
        slot
    }

    /// Compiles `LoadSoundfileLength { var, part }` → `kLoadSoundFieldInt` (fLength).
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — `kInt32` / fLength case.
    pub(super) fn compile_load_soundfile_length(
        &mut self,
        store: &FirStore,
        var: &str,
        part: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        // Push part index onto int stack; executor pops it.
        self.compile_node(store, part)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldInt,
                0, // int_value = 0 → fLength field selector
                R::default(),
                slot as i32, // offset1 = soundfile slot index
                0,
            ));
        Ok(())
    }

    /// Compiles `LoadSoundfileRate { var, part }` → `kLoadSoundFieldInt` (fSR).
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — `kInt32` / fSR case.
    pub(super) fn compile_load_soundfile_rate(
        &mut self,
        store: &FirStore,
        var: &str,
        part: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        self.compile_node(store, part)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldInt,
                1, // int_value = 1 → fSR field selector
                R::default(),
                slot as i32,
                0,
            ));
        Ok(())
    }

    /// Compiles `LoadSoundfileBuffer { var, chan, part, idx }` → `kLoadSoundFieldReal`.
    ///
    /// # Source provenance (C++)
    /// - `visit(LoadSoundfileInst*)` — FAUSTFLOAT buffer case.
    ///
    /// Pushes `chan`, `part`, `idx` onto the int stack; the executor pops them
    /// in reverse order and computes `buffers[chan][offsets[part] + idx]`.
    pub(super) fn compile_load_soundfile_buffer(
        &mut self,
        store: &FirStore,
        var: &str,
        chan: FirId,
        part: FirId,
        idx: FirId,
    ) -> Result<(), CompileError> {
        let slot = self.soundfile_slots.get(var).copied().ok_or_else(|| {
            CompileError::UndeclaredVariable {
                name: var.to_string(),
            }
        })?;
        // Push chan, part, idx — executor pops in LIFO order: idx first, then part, then chan.
        self.compile_node(store, chan)?;
        self.compile_node(store, part)?;
        self.compile_node(store, idx)?;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                FbcOpcode::LoadSoundFieldReal,
                0,
                R::default(),
                slot as i32, // offset1 = soundfile slot index
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(AddMetaDeclareInst*)`.
    pub(super) fn compile_add_meta_declare(
        &mut self,
        var: &str,
        key: &str,
        value: &str,
    ) -> Result<(), CompileError> {
        let offset = if var == "0" {
            -1
        } else {
            self.get_field_offset(var)
        };
        self.ui_instructions
            .push(FbcUiInstruction::declare(offset, key, value));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------
}
