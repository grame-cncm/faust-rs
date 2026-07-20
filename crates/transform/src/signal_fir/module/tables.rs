//! Waveform table, read-table, and write-table lowering.
//!
//! Handles the three table signal families:
//! - `SIGWAVEFORM` — constant ROM tables defined inline in the DSP source;
//! - `SIGRDTBL` — indexed read access into a table;
//! - `SIGWRTBL` / `SIGGEN` — write-driven tables (e.g. delay lines with an
//!   external write index).
//!
//! Also owns table sizing helpers (`table_size_for_signal`) and the index
//! expression normalisation that enforces integer-domain access.
use crate::signal_fir::FirId;
use crate::signal_fir::FirType;
use crate::signal_fir::SigId;
use crate::signal_fir::SignalFirError;
use crate::signal_fir::SignalFirErrorCode;
use crate::signal_fir::module::AccessType;
use crate::signal_fir::module::FirBinOp;
use crate::signal_fir::module::FirBuilder;
use crate::signal_fir::module::SigMatch;
use crate::signal_fir::module::SignalToFirLower;
use crate::signal_fir::module::match_sig;
use crate::signal_fir::siggen::interpret_generator;

impl<'a> SignalToFirLower<'a> {
    /// Lowers a `SIGWAVEFORM` node used as a direct signal output.
    ///
    /// Emits a cycling integer state slot `iWave{N}` (cleared to 0 in
    /// `instanceClear`) that advances by 1 mod `len` each sample, producing the
    /// correct sequential value from the waveform table.
    ///
    /// Contrast with `lower_rdtbl`: when a waveform is used as a read-table
    /// source (via `SIGWRTBL`/`SIGGEN`), the table is filled once in
    /// `ensure_wrtbl_table` and accessed with an arbitrary external index.
    pub(super) fn lower_waveform(
        &mut self,
        node: SigId,
        values: &[SigId],
    ) -> Result<FirId, SignalFirError> {
        let table_name = self.ensure_waveform_table(node, values)?;
        if values.is_empty() {
            return self.unsupported_node(node, "SIGWAVEFORM cannot be empty");
        }
        let n = i32::try_from(values.len()).unwrap_or(i32::MAX);
        let idx_name = format!("iWave{}", node.as_u32());
        if self.sections.named_struct_vars.insert(idx_name.clone()) {
            let mut b = FirBuilder::new(&mut self.store);
            let dec = b.declare_var(idx_name.clone(), FirType::Int32, AccessType::Struct, None);
            self.sections.struct_declarations.push(dec);
            let zero = self.lower_int32_const(0);
            self.register_clear_init(idx_name.clone(), zero);
            // Compute update: iWave = (iWave + 1) % N
            let iwave_load = {
                let mut b = FirBuilder::new(&mut self.store);
                b.load_var(idx_name.clone(), AccessType::Struct, FirType::Int32)
            };
            let one = self.lower_int32_const(1);
            let size = self.lower_int32_const(n);
            let next = {
                let mut b = FirBuilder::new(&mut self.store);
                let sum = b.binop(FirBinOp::Add, iwave_load, one, FirType::Int32);
                b.binop(FirBinOp::Rem, sum, size, FirType::Int32)
            };
            let update = {
                let mut b = FirBuilder::new(&mut self.store);
                b.store_var(idx_name.clone(), AccessType::Struct, next)
            };
            self.regions.current_phases_mut().post_output.push(update);
        }
        let index = {
            let mut b = FirBuilder::new(&mut self.store);
            b.load_var(idx_name, AccessType::Struct, FirType::Int32)
        };
        let real_ty = self.signal_fir_type(node)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_table(table_name, AccessType::Static, index, real_ty))
    }

    /// Lowers one table read by resolving the table producer and normalizing
    /// the runtime read index according to table length.
    pub(super) fn lower_rdtbl(
        &mut self,
        node: SigId,
        tbl: SigId,
        ridx: SigId,
    ) -> Result<FirId, SignalFirError> {
        // Keep C++ `compileSigRDTbl` evaluation order: evaluate table first so
        // pending `wrtbl` side-effects are emitted before read access.
        let _ = self.lower_signal(tbl)?;
        let (table_name, table_len, access) = self.resolve_table(tbl)?;
        if table_len == 0 {
            return self.unsupported_node(node, "SIGRDTBL cannot read an empty table");
        }
        let ridx_sig = ridx;
        let ridx = self.lower_signal(ridx)?;
        let index = self.table_index_with_bounds(ridx, ridx_sig, table_len);
        let real_ty = self.signal_fir_type(node)?;
        let mut b = FirBuilder::new(&mut self.store);
        Ok(b.load_table(table_name, access, index, real_ty))
    }

    /// Lowers one table write producer (`SIGWRTBL`) and returns the table alias.
    ///
    /// Current scope supports deterministic constant-size tables with generator
    /// expansion handled by [`Self::expand_generator_values`].
    pub(super) fn lower_wrtbl(
        &mut self,
        node: SigId,
        _size: SigId,
        generator: SigId,
        widx: SigId,
        wsig: SigId,
    ) -> Result<FirId, SignalFirError> {
        let (table_name, table_len, access) = self.resolve_table(node)?;
        if table_len == 0 {
            return self.unsupported_node(generator, "SIGWRTBL cannot write an empty table");
        }
        if self.arena.is_nil(widx) {
            if self.arena.is_nil(wsig) {
                return self.zero_value_for_signal(node);
            }
            return self.lower_signal(wsig);
        }
        if self.arena.is_nil(wsig) {
            return self.unsupported_node(node, "SIGWRTBL write requires wsig when widx is set");
        }
        let wsig_value = self.lower_signal(wsig)?;
        let widx = self.lower_signal(widx)?;
        let index = self.normalized_table_index(widx, table_len);
        let mut b = FirBuilder::new(&mut self.store);
        self.regions
            .current_phases_mut()
            .immediate
            .push(b.store_table(table_name, access, index, wsig_value));
        Ok(wsig_value)
    }

    /// Resolves a table-producing signal into `(table_name, table_len, access)`.
    ///
    /// Three cases are handled:
    /// - `SIGWAVEFORM`: static constant table (`AccessType::Static`).
    /// - `SIGWRTBL(size, gen, nil, nil)`: read-only generated table, expanded
    ///   at compile-time (`AccessType::Static`).
    /// - `SIGWRTBL(size, gen, widx, wsig)`: writable runtime table; written
    ///   per-sample and read with (`AccessType::Struct`).
    pub(super) fn resolve_table(
        &mut self,
        sig: SigId,
    ) -> Result<(String, usize, AccessType), SignalFirError> {
        if let Some(name) = self.ui.waveform_tables.get(&sig).cloned() {
            let len = self.ui.waveform_table_len.get(&sig).copied().unwrap_or(0);
            let access = self
                .ui
                .table_access_by_sig
                .get(&sig)
                .copied()
                .unwrap_or(AccessType::Static);
            return Ok((name, len, access));
        }
        match match_sig(self.arena, sig) {
            SigMatch::Waveform(values) => {
                let name = self.ensure_waveform_table(sig, values)?;
                Ok((name, values.len(), AccessType::Static))
            }
            SigMatch::WrTbl(size, generator, widx, wsig) => {
                if self.arena.is_nil(widx) && self.arena.is_nil(wsig) {
                    let (name, len) = self.ensure_readonly_table(sig, size, generator)?;
                    Ok((name, len, AccessType::Static))
                } else {
                    let (name, len) = self.ensure_wrtbl_table(sig, size, generator)?;
                    Ok((name, len, AccessType::Struct))
                }
            }
            _ => self.unsupported_node(
                sig,
                "table access currently supports SIGWAVEFORM and SIGWRTBL forms in Step 2H",
            ),
        }
    }

    /// Ensures one waveform table declaration is emitted exactly once.
    pub(super) fn ensure_waveform_table(
        &mut self,
        sig: SigId,
        values: &[SigId],
    ) -> Result<String, SignalFirError> {
        if let Some(name) = self.ui.waveform_tables.get(&sig).cloned() {
            return Ok(name);
        }
        let mut lowered_values = Vec::with_capacity(values.len());
        for value in values {
            lowered_values.push(self.lower_signal(*value)?);
        }
        let elem_ty = self.signal_fir_type(sig)?;
        let prefix = if elem_ty == FirType::Int32 {
            "iTbl"
        } else {
            "fTbl"
        };
        let name = format!("{prefix}{}", sig.as_u32());
        let mut b = FirBuilder::new(&mut self.store);
        let decl = b.declare_table(name.clone(), AccessType::Static, elem_ty, &lowered_values);
        self.sections.static_declarations.push(decl);
        self.ui.waveform_tables.insert(sig, name.clone());
        self.ui.waveform_table_len.insert(sig, values.len());
        self.ui.table_access_by_sig.insert(sig, AccessType::Static);
        Ok(name)
    }

    /// Ensures one read-only `rdtable`-style declaration is emitted exactly once.
    ///
    /// Unlike `ensure_waveform_table` (literal constant values), this expands
    /// the generator at compile-time via `expand_generator_values`.  The
    /// resulting array is declared `Static` — no per-instance write is needed.
    pub(super) fn ensure_readonly_table(
        &mut self,
        sig: SigId,
        size_sig: SigId,
        generator_sig: SigId,
    ) -> Result<(String, usize), SignalFirError> {
        let size = self.table_size_from_sig(size_sig)?;
        let elem_ty = self.signal_fir_type(sig)?;
        let generated = self.expand_generator_values(generator_sig, size, &elem_ty)?;
        let prefix = if elem_ty == FirType::Int32 {
            "iTbl"
        } else {
            "fTbl"
        };
        let name = format!("{prefix}{}", sig.as_u32());
        let mut b = FirBuilder::new(&mut self.store);
        let decl = b.declare_table(name.clone(), AccessType::Static, elem_ty, &generated);
        self.sections.static_declarations.push(decl);
        self.ui.waveform_tables.insert(sig, name.clone());
        self.ui.waveform_table_len.insert(sig, size);
        self.ui.table_access_by_sig.insert(sig, AccessType::Static);
        Ok((name, size))
    }

    /// Ensures one writable `rwtable` declaration and per-instance
    /// initialization are emitted exactly once.
    ///
    /// The table lives in the DSP struct (`AccessType::Struct`) so it can be
    /// written at runtime.  The generator is expanded at compile-time and
    /// registered in `instanceConstants` to seed initial values; per-sample
    /// writes are emitted by `lower_wrtbl` into the sample loop immediate phase.
    pub(super) fn ensure_wrtbl_table(
        &mut self,
        sig: SigId,
        size_sig: SigId,
        generator_sig: SigId,
    ) -> Result<(String, usize), SignalFirError> {
        let size = self.table_size_from_sig(size_sig)?;
        let elem_ty = self.signal_fir_type(sig)?;
        let generated = self.expand_generator_values(generator_sig, size, &elem_ty)?;
        let prefix = if elem_ty == FirType::Int32 {
            "iTbl"
        } else {
            "fTbl"
        };
        let name = format!("{prefix}{}", sig.as_u32());
        let mut b = FirBuilder::new(&mut self.store);
        let decl = b.declare_table(
            name.clone(),
            AccessType::Struct,
            elem_ty.clone(),
            &generated,
        );
        self.sections.struct_declarations.push(decl);
        self.register_constant_table_init(name.clone(), AccessType::Struct, elem_ty, &generated);
        self.ui.waveform_tables.insert(sig, name.clone());
        self.ui.waveform_table_len.insert(sig, size);
        self.ui.table_access_by_sig.insert(sig, AccessType::Struct);
        Ok((name, size))
    }

    /// Evaluates table-size signal to a positive `usize`.
    pub(super) fn table_size_from_sig(&self, size_sig: SigId) -> Result<usize, SignalFirError> {
        match match_sig(self.arena, size_sig) {
            SigMatch::Int(v) if v > 0 => usize::try_from(v).map_err(|_| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("SIGWRTBL size conversion overflow: {v}"),
                )
            }),
            SigMatch::Int(v) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("SIGWRTBL size must be > 0, got {v}"),
            )),
            _ => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                "SIGWRTBL currently requires constant integer size in Step 2H",
            )),
        }
    }

    /// Expands a table generator signal into concrete initializer values.
    ///
    /// Only generator shapes that can be fully resolved at compile-time are
    /// accepted in the current fast-lane slice.
    pub(super) fn expand_generator_values(
        &mut self,
        generator_sig: SigId,
        size: usize,
        elem_ty: &FirType,
    ) -> Result<Vec<FirId>, SignalFirError> {
        let init_sig = if let SigMatch::Gen(inner) = match_sig(self.arena, generator_sig) {
            inner
        } else {
            generator_sig
        };
        match match_sig(self.arena, init_sig) {
            SigMatch::Waveform(values) => {
                if values.is_empty() {
                    return Err(SignalFirError::new(
                        SignalFirErrorCode::UnsupportedSignalNode,
                        "SIGGEN waveform cannot be empty in Step 2H",
                    ));
                }
                let mut out = Vec::with_capacity(size);
                for index in 0..size {
                    let item = values[index % values.len()];
                    out.push(self.lower_signal(item)?);
                }
                Ok(out)
            }
            SigMatch::Int(_) | SigMatch::Real(_) => {
                let v = self.lower_signal(init_sig)?;
                Ok(vec![v; size])
            }
            _ => {
                // Computed generator: interpret at compile time.
                // This is the compile-time equivalent of C++'s signal2Container
                // approach — since SIGGEN generators are always 0-input
                // deterministic DSP, we can evaluate them directly.
                let values = interpret_generator(self.arena, init_sig, size)?;
                let mut out = Vec::with_capacity(size);
                for v in values {
                    out.push(self.fir_const_for_table_value(v, elem_ty)?);
                }
                Ok(out)
            }
        }
    }

    /// Converts one compile-time generator sample into the declared FIR table
    /// element type, preserving integer tables as `Int32` and real tables at
    /// the current internal precision.
    pub(super) fn fir_const_for_table_value(
        &mut self,
        value: f64,
        elem_ty: &FirType,
    ) -> Result<FirId, SignalFirError> {
        let mut b = FirBuilder::new(&mut self.store);
        match elem_ty {
            FirType::Int32 => Ok(b.int32(value as i32)),
            FirType::Float32 => Ok(b.float32(value as f32)),
            FirType::Float64 => Ok(b.float64(value)),
            other => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("unsupported table element type for generator expansion: {other:?}"),
            )),
        }
    }

    /// Normalizes one table index to `[0, table_len)` with integer modulo.
    /// Wraps a table index with `((index % size) + size) % size` to produce a
    /// non-negative in-bounds `Int32` offset.
    ///
    /// The promoter guarantees that all table index signals are Int-typed
    /// (wrapped in `IntCast` if necessary), so `index` is already `Int32` at the
    /// FIR level when this function is called.  No cast is needed.
    pub(super) fn normalized_table_index(&mut self, index: FirId, table_len: usize) -> FirId {
        let size = {
            let mut b = FirBuilder::new(&mut self.store);
            b.int32(i32::try_from(table_len).unwrap_or(i32::MAX))
        };
        let rem = {
            let mut b = FirBuilder::new(&mut self.store);
            b.binop(FirBinOp::Rem, index, size, FirType::Int32)
        };
        let rem_plus_size = {
            let mut b = FirBuilder::new(&mut self.store);
            b.binop(FirBinOp::Add, rem, size, FirType::Int32)
        };
        let mut b = FirBuilder::new(&mut self.store);
        b.binop(FirBinOp::Rem, rem_plus_size, size, FirType::Int32)
    }

    /// Selects the appropriate index bounds strategy based on the interval of
    /// `index_sig`:
    ///
    /// - **[lo, hi] ⊆ [0, N-1]**: the interval proves the index is always
    ///   in-bounds → emit direct access (no bounds check).
    /// - **[lo, hi] with lo ≥ 0, hi finite but > N-1**: non-negative but may
    ///   overflow → clamp to `min_i(N-1, index)`.
    /// - **[lo, hi] finite with lo < 0**: signed bounds → full clamp
    ///   `min_i(N-1, max_i(0, index))`.
    /// - **Unknown / infinite interval**: fall back to modular wrapping
    ///   `((index % N) + N) % N`.
    ///
    /// This mirrors the C++ reference compiler's interval-driven access
    /// strategy and avoids the systematic over-conservatism of always applying
    /// modular wrapping.
    pub(super) fn table_index_with_bounds(
        &mut self,
        index_fir: FirId,
        index_sig: SigId,
        table_len: usize,
    ) -> FirId {
        let n = i32::try_from(table_len).unwrap_or(i32::MAX);
        let iv = self.sig_types.get(&index_sig).map(|ty| ty.interval());

        if let Some(iv) = iv {
            let lo = iv.lo();
            let hi = iv.hi();
            if lo.is_finite() && hi.is_finite() {
                let lo_i = lo as i64;
                let hi_i = hi as i64;
                let n_i = n as i64;
                if lo_i >= 0 && hi_i >= 0 && hi_i < n_i {
                    // Index is already provably in [0, N-1] — direct access.
                    return index_fir;
                }
                if lo_i >= 0 {
                    // Non-negative but hi may exceed N-1 — upper clamp only.
                    let upper = self.lower_int32_const(n - 1);
                    self.used_protos.int_fun_names.insert("min_i");
                    let mut b = FirBuilder::new(&mut self.store);
                    return b.fun_call("min_i", &[index_fir, upper], FirType::Int32);
                }
                // Signed bounds — full clamp max(0, min(N-1, x)).
                let zero = self.lower_int32_const(0);
                let upper = self.lower_int32_const(n - 1);
                self.used_protos.int_fun_names.insert("min_i");
                self.used_protos.int_fun_names.insert("max_i");
                let clamped = {
                    let mut b = FirBuilder::new(&mut self.store);
                    b.fun_call("min_i", &[upper, index_fir], FirType::Int32)
                };
                let mut b = FirBuilder::new(&mut self.store);
                return b.fun_call("max_i", &[clamped, zero], FirType::Int32);
            }
        }
        // No interval info or infinite bounds — full modular wrapping.
        self.normalized_table_index(index_fir, table_len)
    }
}
