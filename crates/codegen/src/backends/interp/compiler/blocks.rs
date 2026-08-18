//! `blocks` half of the FBC compiler.
//!
//! FIR block and statement-list compilation: the entry that walks a node and the block-level scaffolding around it.
//!
//! Split out of `compiler.rs` on 2026-08-18, where all 54 methods sat in one
//! 1891-line `impl`. The method bodies are moved verbatim; only their
//! visibility widened from private to `pub(super)` so the sibling modules can
//! still reach them.

use super::*;

impl<R: FbcReal> FirToFbcCompiler<R> {
    /// Compiles a single FIR node (and its transitive children) into
    /// FBC bytecode in the current block.
    ///
    /// Dispatch is intentionally exhaustive over [`FirMatch`]: unlike the C++
    /// visitor hierarchy, unsupported nodes are surfaced as typed Rust errors
    /// instead of falling through to `faustassert(false)`.
    pub fn compile_node(&mut self, store: &FirStore, id: FirId) -> Result<(), CompileError> {
        match match_fir(store, id) {
            // --- Values ---
            FirMatch::Int32 { value, .. } => self.compile_int32(value),
            FirMatch::Float32 { value, .. } => self.compile_float32(value),
            FirMatch::Float64 { value, .. } => self.compile_float64(value),
            FirMatch::Bool { value, .. } => self.compile_bool(value),

            // --- Variables ---
            FirMatch::LoadVar {
                ref name,
                access,
                ref typ,
            } => self.compile_load_var(store, name, access, typ),
            FirMatch::LoadTable {
                ref name,
                access,
                index,
                ref typ,
            } => self.compile_load_table(store, name, access, index, typ),
            FirMatch::LoadVarAddress { .. } => Err(CompileError::LoadVarAddressNotSupported),
            FirMatch::TeeVar {
                ref name,
                access,
                value,
                ref typ,
            } => self.compile_tee_var(store, name, access, value, typ),

            // --- Declarations ---
            FirMatch::DeclareVar {
                ref name,
                ref typ,
                access,
                init,
            } => self.compile_declare_var(store, name, typ, access, init),
            FirMatch::DeclareFun { .. } => Ok(()),
            FirMatch::DeclareStructType { .. } => Ok(()),

            // --- Storage ---
            FirMatch::StoreVar {
                ref name,
                access,
                value,
            } => self.compile_store_var(store, name, access, value),
            FirMatch::StoreTable {
                ref name,
                access,
                index,
                value,
            } => self.compile_store_table(store, name, access, index, value),
            FirMatch::ShiftArrayVar {
                ref name, delay, ..
            } => self.compile_shift_array(name, delay),
            FirMatch::Drop(inner) => self.compile_node(store, inner),

            // --- Arithmetic ---
            FirMatch::BinOp { op, lhs, rhs, .. } => self.compile_binop(store, op, lhs, rhs),
            FirMatch::Neg { value, ref typ } => self.compile_neg(store, value, typ),

            // --- Cast ---
            FirMatch::Cast { ref typ, value } => self.compile_cast(store, typ, value),
            FirMatch::Bitcast { ref typ, value } => self.compile_bitcast(store, typ, value),

            // --- Control flow ---
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => self.compile_select2(store, cond, then_value, else_value),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => self.compile_if(store, cond, then_block, else_block),
            FirMatch::Switch {
                cond,
                ref cases,
                default,
            } => self.compile_switch(store, cond, cases, default),
            FirMatch::ForLoop {
                ref var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => self.compile_for_loop(
                store,
                ForLoopParams {
                    var,
                    init,
                    end,
                    step,
                    body,
                    is_reverse,
                },
            ),
            FirMatch::SimpleForLoop {
                ref var,
                upper,
                body,
                is_reverse,
                ..
            } => self.compile_simple_for_loop(store, var, upper, body, is_reverse),
            FirMatch::Block(ref stmts) => {
                let stmts = stmts.clone();
                self.compile_block(store, &stmts)
            }

            // --- Function calls ---
            FirMatch::FunCall {
                ref name,
                ref args,
                ref typ,
            } => {
                let name = name.clone();
                let args = args.clone();
                let typ = typ.clone();
                self.compile_fun_call(store, &name, &args, &typ)
            }

            // --- UI ---
            FirMatch::OpenBox { ref typ, ref label } => self.compile_open_box(typ, label),
            FirMatch::CloseBox => self.compile_close_box(),
            FirMatch::AddButton {
                ref typ,
                ref label,
                ref var,
            } => self.compile_add_button(typ, label, var),
            FirMatch::AddSlider {
                ref typ,
                ref label,
                ref var,
                init,
                lo,
                hi,
                step,
            } => self.compile_add_slider(typ, label, var, init, lo, hi, step),
            FirMatch::AddBargraph {
                ref typ,
                ref label,
                ref var,
                lo,
                hi,
            } => self.compile_add_bargraph(typ, label, var, lo, hi),
            FirMatch::AddSoundfile {
                ref label,
                ref url,
                ref var,
            } => self.compile_add_soundfile(label, url, var),

            // --- Soundfile access ---
            FirMatch::LoadSoundfileLength { ref var, part } => {
                let var = var.clone();
                self.compile_load_soundfile_length(store, &var, part)
            }
            FirMatch::LoadSoundfileRate { ref var, part } => {
                let var = var.clone();
                self.compile_load_soundfile_rate(store, &var, part)
            }
            FirMatch::LoadSoundfileBuffer {
                ref var,
                chan,
                part,
                idx,
                ..
            } => {
                let var = var.clone();
                self.compile_load_soundfile_buffer(store, &var, chan, part, idx)
            }

            FirMatch::AddMetaDeclare {
                ref var,
                ref key,
                ref value,
            } => self.compile_add_meta_declare(var, key, value),

            // --- No-ops ---
            FirMatch::NullStatement
            | FirMatch::Label(_)
            | FirMatch::Return(_)
            | FirMatch::Int32Array { .. }
            | FirMatch::Float32Array { .. }
            | FirMatch::Float64Array { .. } => Ok(()),

            // --- Unsupported ---
            other => Err(CompileError::UnsupportedNode {
                description: format!("{other:?}"),
            }),
        }
    }

    /// Compiles a FIR block node as a new sub-block in the arena and returns
    /// its allocated [`BlockId`].
    ///
    /// If `block_id` does not decode as a [`FirMatch::Block`], an empty block
    /// (containing only `kReturn`) is emitted.
    ///
    /// This is the building block for [`super::generate_interp_module`] which compiles
    /// each named DSP section (init, compute, …) into a separate arena block.
    pub fn compile_fir_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<BlockId, CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => vec![],
        };
        self.begin_sub_block();
        for id in &nodes {
            self.compile_node(store, *id)?;
        }
        Ok(self.end_sub_block())
    }

    /// Compiles a list of FIR statements as a new sub-block in the arena.
    ///
    /// This is used by the interpreter backend to split a single FIR `compute`
    /// body into a control prefix block and a DSP loop block without inventing
    /// extra FIR declarations.
    pub fn compile_fir_stmt_list_block(
        &mut self,
        store: &FirStore,
        stmts: &[FirId],
    ) -> Result<BlockId, CompileError> {
        self.begin_sub_block();
        for &id in stmts {
            self.compile_node(store, id)?;
        }
        Ok(self.end_sub_block())
    }

    /// Pre-declares storage nodes from a FIR `Block` into the heap layout.
    ///
    /// This allocates entries in [`Self::field_table`] for top-level module
    /// storage (`dsp_struct`, `globals`) without emitting executable bytecode.
    /// The interpreter backend uses this to make struct/global fields visible
    /// before compiling function bodies that reference them.
    ///
    /// Only direct `DeclareVar` / `DeclareTable` items are accepted; other
    /// nodes are ignored so prototype-only `DeclareFun` entries in `globals`
    /// can coexist with storage declarations.
    pub fn predeclare_storage_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<(), CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => return Ok(()),
        };
        for id in nodes {
            match match_fir(store, id) {
                FirMatch::DeclareVar {
                    ref name, ref typ, ..
                } => self.predeclare_var_storage(name, typ),
                FirMatch::DeclareTable {
                    ref name,
                    ref elem_type,
                    ref values,
                    ..
                } => self.predeclare_table_storage(name, elem_type, values.len() as i32),
                FirMatch::DeclareFun { .. } | FirMatch::DeclareStructType { .. } => {}
                _ => {}
            }
        }
        Ok(())
    }

    /// Compiles bulk-initialization bytecode for file-scope `const static`
    /// tables declared in the `static_decls` FIR module block.
    ///
    /// In the C/C++ backends these tables are emitted as file-scope arrays with
    /// inline initialisers (`const static float fTbl[N] = {…}`) and require no
    /// runtime initialization. In the interpreter every value lives on the int
    /// or real heap, so the constant data must be written there before
    /// `compute()` runs.
    ///
    /// This method walks the `static_decls` block, allocates heap storage for
    /// each `DeclareTable` (idempotent — `predeclare_storage_block` may have
    /// already done it), and emits one `BlockStoreInt` or `BlockStoreReal`
    /// instruction per table that bulk-copies the constant element values.
    ///
    /// The returned block should be prepended to (or used as) the
    /// `staticInit` factory block so the data is in place before the first
    /// call to `compute()`.
    pub fn compile_static_decls_init_block(
        &mut self,
        store: &FirStore,
        block_id: FirId,
    ) -> Result<BlockId, CompileError> {
        let nodes = match match_fir(store, block_id) {
            FirMatch::Block(ids) => ids,
            _ => return Ok(self.alloc_empty_block()),
        };
        self.begin_sub_block();
        for id in nodes {
            if let FirMatch::DeclareTable {
                ref name,
                ref elem_type,
                ref values,
                ..
            } = match_fir(store, id)
            {
                if values.is_empty() {
                    continue;
                }
                // Ensure the table is registered in the heap layout.
                self.predeclare_table_storage(name, elem_type, values.len() as i32);
                let desc = self.field_table[name.as_str()].clone();
                if desc.heap_type == HeapType::Int {
                    let data: Vec<i32> = values
                        .iter()
                        .filter_map(|&v| {
                            if let FirMatch::Int32 { value, .. } = match_fir(store, v) {
                                Some(value)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if data.len() == values.len() {
                        let len = data.len() as i32;
                        self.current_block.push_block_store(
                            FbcInstruction::with_values_and_offsets(
                                FbcOpcode::BlockStoreInt,
                                0,
                                R::default(),
                                desc.offset,
                                len,
                            ),
                            BlockStoreData::Int(data),
                        );
                    }
                } else {
                    let data: Vec<R> = values
                        .iter()
                        .filter_map(|&v| match match_fir(store, v) {
                            FirMatch::Float32 { value, .. } => Some(R::from_f64(f64::from(value))),
                            FirMatch::Float64 { value, .. } => Some(R::from_f64(value)),
                            _ => None,
                        })
                        .collect();
                    if data.len() == values.len() {
                        let len = data.len() as i32;
                        self.current_block.push_block_store(
                            FbcInstruction::with_values_and_offsets(
                                FbcOpcode::BlockStoreReal,
                                0,
                                R::default(),
                                desc.offset,
                                len,
                            ),
                            BlockStoreData::Real(data),
                        );
                    }
                }
            }
        }
        Ok(self.end_sub_block())
    }

    /// # Source provenance (C++)
    /// - `visit(BlockInst*)` — iterates over block statements.
    pub(super) fn compile_block(
        &mut self,
        store: &FirStore,
        stmts: &[FirId],
    ) -> Result<(), CompileError> {
        for &stmt in stmts {
            self.compile_node(store, stmt)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Function calls
    // -----------------------------------------------------------------------
}
