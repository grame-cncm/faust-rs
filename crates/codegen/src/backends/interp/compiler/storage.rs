//! `storage` half of the FBC compiler.
//!
//! Literals, variables, and tables: loads, stores, declarations, and the storage descriptors behind them.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
    /// # Source provenance (C++)
    /// - `visit(Int32NumInst*)` — pushes integer onto int stack.
    pub(super) fn compile_int32(&mut self, value: i32) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            value,
            R::default(),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(FloatNumInst*)` — pushes real onto real stack.
    pub(super) fn compile_float32(&mut self, value: f32) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            R::from_f64(f64::from(value)),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(DoubleNumInst*)` — pushes real onto real stack.
    pub(super) fn compile_float64(&mut self, value: f64) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::RealValue,
            0,
            R::from_f64(value),
        ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(BoolNumInst*)` — pushes 0 or 1 onto int stack.
    pub(super) fn compile_bool(&mut self, value: bool) -> Result<(), CompileError> {
        self.current_block.push(FbcInstruction::with_values(
            FbcOpcode::Int32Value,
            i32::from(value),
            R::default(),
        ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Variable access
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(LoadVarInst*)` — named address path.
    pub(super) fn compile_load_var(
        &mut self,
        _store: &FirStore,
        name: &str,
        access: AccessType,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        if access == AccessType::FunArgs
            && name == "sample_rate"
            && !self.field_table.contains_key(name)
        {
            let desc = self
                .field_table
                .get("fSampleRate")
                .or_else(|| self.field_table.get("fSamplingFreq"))
                .or_else(|| self.field_table.get("fSamplingRate"))
                .cloned()
                .unwrap_or_else(|| {
                    let offset = self.int_heap_offset;
                    self.int_heap_offset += 1;
                    MemoryDesc {
                        offset,
                        size: 1,
                        heap_type: HeapType::Int,
                    }
                });
            self.field_table.insert(name.to_string(), desc);
        }
        if access == AccessType::FunArgs && name == "count" && !self.field_table.contains_key(name)
        {
            // Reserve a stable int heap slot for the runtime-set `count` pseudo argument.
            let offset = self.int_heap_offset;
            self.int_heap_offset += 1;
            self.field_table.insert(
                name.to_string(),
                MemoryDesc {
                    offset,
                    size: 1,
                    heap_type: HeapType::Int,
                },
            );
        }
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::LoadInt
        } else {
            FbcOpcode::LoadReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(LoadVarInst*)` — indexed address path.
    pub(super) fn compile_load_table(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        index: FirId,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        // Compile the index expression first (pushes onto int stack).
        self.compile_node(store, index)?;

        // Special handling for input channels.
        if let Some(channel) = parse_io_channel(name, "input") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadInput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }
        if let Some(channel) = parse_io_channel(name, "output") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::LoadOutput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::LoadIndexedInt
        } else {
            FbcOpcode::LoadIndexedReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                desc.size,
            ));
        Ok(())
    }

    /// Compile `TeeVar` — store to variable and leave value on stack.
    pub(super) fn compile_tee_var(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        value: FirId,
        _typ: &FirType,
    ) -> Result<(), CompileError> {
        // Compile value, store it, then reload (store+load = tee).
        self.compile_node(store, value)?;
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let (store_op, load_op) = if desc.heap_type == HeapType::Int {
            (FbcOpcode::StoreInt, FbcOpcode::LoadInt)
        } else {
            (FbcOpcode::StoreReal, FbcOpcode::LoadReal)
        };
        let offset = desc.offset;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                store_op,
                0,
                R::default(),
                offset,
                0,
            ));
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                load_op,
                0,
                R::default(),
                offset,
                0,
            ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Declarations
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(DeclareVarInst*)` — allocates heap slots and optionally
    ///   compiles the initializer.
    pub(super) fn compile_declare_var(
        &mut self,
        store: &FirStore,
        name: &str,
        typ: &FirType,
        _access: AccessType,
        init: Option<FirId>,
    ) -> Result<(), CompileError> {
        // Skip input/output pseudo-variables.
        if name.starts_with("input") || name.starts_with("output") {
            return Ok(());
        }

        // Determine element type and array size.
        let (elem_type, array_size) = match typ {
            FirType::Array(elem, size) => (elem.as_ref(), *size as i32),
            _ => (typ, 1),
        };

        // Soundfile handles get a slot index, not a heap slot.
        if matches!(elem_type, FirType::Sound) {
            self.alloc_soundfile_slot(name);
            return Ok(());
        }

        self.alloc_storage_desc(name, elem_type, array_size);

        // Compile initializer if present.
        if let Some(init_id) = init {
            self.compile_init_store(store, name, typ, init_id)?;
        }
        Ok(())
    }

    /// Compiles the initializer for a `DeclareVar`.
    ///
    /// # Source provenance (C++)
    /// - `visitStore(inst->fAddress, inst->fValue, inst->fType)` called
    ///   from `visit(DeclareVarInst*)`.
    pub(super) fn compile_init_store(
        &mut self,
        store: &FirStore,
        name: &str,
        typ: &FirType,
        init_id: FirId,
    ) -> Result<(), CompileError> {
        let desc = self.field_table[name].clone();

        // Array waveform store path.
        if let FirType::Array(_, _) = typ {
            match match_fir(store, init_id) {
                FirMatch::Int32Array { values, .. } => {
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreInt,
                            0,
                            R::default(),
                            desc.offset,
                            values.len() as i32,
                        ),
                        BlockStoreData::Int(values),
                    );
                    return Ok(());
                }
                FirMatch::Float32Array { values, .. } => {
                    let data: Vec<R> = values.iter().map(|v| R::from_f64(f64::from(*v))).collect();
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreReal,
                            0,
                            R::default(),
                            desc.offset,
                            data.len() as i32,
                        ),
                        BlockStoreData::Real(data),
                    );
                    return Ok(());
                }
                FirMatch::Float64Array { values, .. } => {
                    let data: Vec<R> = values.iter().map(|v| R::from_f64(*v)).collect();
                    self.current_block.push_block_store(
                        FbcInstruction::with_values_and_offsets(
                            FbcOpcode::BlockStoreReal,
                            0,
                            R::default(),
                            desc.offset,
                            data.len() as i32,
                        ),
                        BlockStoreData::Real(data),
                    );
                    return Ok(());
                }
                _ => {
                    // Fall through to scalar store.
                }
            }
        }

        // Scalar store path: compile value, then emit StoreInt/StoreReal.
        self.compile_node(store, init_id)?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreInt
        } else {
            FbcOpcode::StoreReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                0,
            ));
        Ok(())
    }

    /// Reserves heap storage for a scalar/array variable declaration without
    /// compiling its initializer.
    pub(super) fn predeclare_var_storage(&mut self, name: &str, typ: &FirType) {
        if name.starts_with("input") || name.starts_with("output") {
            return;
        }
        let (elem_type, array_size) = match typ {
            FirType::Array(elem, size) => (elem.as_ref(), *size as i32),
            _ => (typ, 1),
        };
        // Soundfile handles get a slot index, not a heap slot.
        if matches!(elem_type, FirType::Sound) {
            self.alloc_soundfile_slot(name);
            return;
        }
        let _ = self.alloc_storage_desc(name, elem_type, array_size);
    }

    /// Reserves heap storage for a table declaration without compiling values.
    pub(super) fn predeclare_table_storage(&mut self, name: &str, elem_type: &FirType, size: i32) {
        if name.starts_with("input") || name.starts_with("output") {
            return;
        }
        let _ = self.alloc_storage_desc(name, elem_type, size.max(0));
    }

    /// Allocates (or reuses) a memory descriptor in the compiler heap layout.
    ///
    /// If the name already exists, the previous descriptor is preserved so
    /// repeated pre-declaration/compilation passes remain idempotent.
    pub(super) fn alloc_storage_desc(
        &mut self,
        name: &str,
        elem_type: &FirType,
        array_size: i32,
    ) -> MemoryDesc {
        if let Some(existing) = self.field_table.get(name) {
            return existing.clone();
        }
        let heap_type = if is_int_type(elem_type) {
            HeapType::Int
        } else {
            HeapType::Real
        };
        let offset = if heap_type == HeapType::Int {
            let o = self.int_heap_offset;
            self.int_heap_offset += array_size;
            o
        } else {
            let o = self.real_heap_offset;
            self.real_heap_offset += array_size;
            o
        };
        let desc = MemoryDesc {
            offset,
            size: array_size,
            heap_type,
        };
        self.field_table.insert(name.to_string(), desc.clone());
        desc
    }

    // -----------------------------------------------------------------------
    // Storage
    // -----------------------------------------------------------------------

    /// # Source provenance (C++)
    /// - `visit(StoreVarInst*)` / `visitStore()` — named address path.
    pub(super) fn compile_store_var(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        value: FirId,
    ) -> Result<(), CompileError> {
        // Compile value (pushes onto stack).
        self.compile_node(store, value)?;

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreInt
        } else {
            FbcOpcode::StoreReal
        };
        let offset = desc.offset;
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                offset,
                0,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visitStore()` — indexed address path.
    pub(super) fn compile_store_table(
        &mut self,
        store: &FirStore,
        name: &str,
        _access: AccessType,
        index: FirId,
        value: FirId,
    ) -> Result<(), CompileError> {
        // Compile value first, then index (matches C++ order).
        self.compile_node(store, value)?;
        self.compile_node(store, index)?;

        // Special handling for output channels.
        if let Some(channel) = parse_io_channel(name, "output") {
            self.current_block
                .push(FbcInstruction::with_values_and_offsets(
                    FbcOpcode::StoreOutput,
                    0,
                    R::default(),
                    channel,
                    0,
                ));
            return Ok(());
        }

        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::StoreIndexedInt
        } else {
            FbcOpcode::StoreIndexedReal
        };
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset,
                desc.size,
            ));
        Ok(())
    }

    /// # Source provenance (C++)
    /// - `visit(ShiftArrayVarInst*)`.
    pub(super) fn compile_shift_array(
        &mut self,
        name: &str,
        delay: i32,
    ) -> Result<(), CompileError> {
        let desc = self
            .field_table
            .get(name)
            .ok_or_else(|| CompileError::UndeclaredVariable {
                name: name.to_string(),
            })?;
        let opcode = if desc.heap_type == HeapType::Int {
            FbcOpcode::BlockShiftInt
        } else {
            FbcOpcode::BlockShiftReal
        };
        // C++: offset1 = tmp.fOffset + inst->fDelay, offset2 = tmp.fOffset
        self.current_block
            .push(FbcInstruction::with_values_and_offsets(
                opcode,
                0,
                R::default(),
                desc.offset + delay,
                desc.offset,
            ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Arithmetic
    // -----------------------------------------------------------------------
}
