//! Borrow-bundle contexts for delay-line FIR emission.
//!
//! Holds [`DelayFirCtx`] (allocation-time context assembled from disjoint
//! `SignalToFirLower` fields) and [`DelayLoweringCtx`] (lowering-time context
//! passed to per-strategy emit functions).

use std::collections::{HashMap, HashSet};

use fir::{AccessType, FirBuilder, FirId, FirStore, FirType};
use signals::SigId;

use crate::signal_prepare::SimpleSigType;

use super::{SignalFirError, SignalFirErrorCode};

// ─── DelayFirCtx ─────────────────────────────────────────────────────────────

/// Borrowed context bundle for delay-line FIR emission.
///
/// Assembled from disjoint fields of `SignalToFirLower` using Rust's field-level
/// split-borrow facility.  Because the `delay: DelayManager` field of
/// `SignalToFirLower` is NOT included here, callers can hold both a
/// `&mut DelayManager` and a `&mut DelayFirCtx` simultaneously.
///
/// # Construction
///
/// Construct via an explicit struct literal at each call site in `module/`:
///
/// ```rust,ignore
/// let mut ctx = DelayFirCtx {
///     store: &mut self.store,
///     real_ty: self.real_ty.clone(),
///     types: self.types,
///     struct_declarations: &mut self.struct_declarations,
///     clear_statements: &mut self.clear_statements,
///     clear_init_seen: &mut self.clear_init_seen,
///     next_loop_var_id: &mut self.next_loop_var_id,
///     uses_iota: &mut self.uses_iota,
/// };
/// self.delay.ensure_delay_line(carried, delay, &mut ctx)?;
/// ```
///
/// **Do not** construct via a `&mut self` method call — that would borrow all of
/// `self` and prevent the simultaneous borrow of `self.delay`.
pub(crate) struct DelayFirCtx<'a> {
    pub(crate) store: &'a mut FirStore,
    pub(crate) real_ty: FirType,
    pub(crate) types: &'a HashMap<SigId, SimpleSigType>,
    pub(crate) struct_declarations: &'a mut Vec<FirId>,
    pub(crate) clear_statements: &'a mut Vec<FirId>,
    pub(crate) clear_init_seen: &'a mut HashSet<String>,
    pub(crate) next_loop_var_id: &'a mut usize,
    pub(crate) uses_iota: &'a mut bool,
}

impl<'a> DelayFirCtx<'a> {
    /// Returns the FIR element type for a delay-line carrier signal.
    pub(crate) fn signal_elem_type(&self, carried: SigId) -> Result<FirType, SignalFirError> {
        match self.types.get(&carried) {
            Some(SimpleSigType::Int) => Ok(FirType::Int32),
            Some(SimpleSigType::Real) => Ok(self.real_ty.clone()),
            Some(SimpleSigType::Sound) => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "signal {} cannot use a soundfile handle as delay-line element type",
                    carried.as_u32()
                ),
            )),
            None => Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("missing prepared type for signal {}", carried.as_u32()),
            )),
        }
    }

    /// Generates a fresh loop-variable name using the shared monotonic counter.
    pub(crate) fn fresh_loop_var(&mut self, prefix: &str) -> String {
        fir::helpers::fresh_loop_var(self.next_loop_var_id, prefix)
    }

    /// Declares the per-line `fIdx<id>` counter for an `IfWrapping` delay line,
    /// idempotent.
    ///
    /// Emits the struct declaration and an `instanceClear` assignment `counter = 0`.
    pub(crate) fn ensure_if_wrapping_counter(&mut self, counter_name: String) {
        if !self.clear_init_seen.insert(counter_name.clone()) {
            return;
        }
        let zero = {
            let mut b = FirBuilder::new(self.store);
            b.int32(0)
        };
        let decl = {
            let mut b = FirBuilder::new(self.store);
            b.declare_var(
                counter_name.clone(),
                FirType::Int32,
                AccessType::Struct,
                None,
            )
        };
        self.struct_declarations.push(decl);
        let mut b = FirBuilder::new(self.store);
        self.clear_statements
            .push(b.store_var(counter_name, AccessType::Struct, zero));
    }

    /// Emits an `instanceClear` zeroing loop for a delay-line array, idempotent.
    ///
    /// Uses `clear_init_seen` for deduplication.  The element zero value is
    /// derived from `sig`'s `SimpleSigType`: `Int32` → `0i32`, `Real` → `0.0`.
    pub(crate) fn register_delay_clear(
        &mut self,
        name: String,
        size: usize,
        sig: SigId,
    ) -> Result<(), SignalFirError> {
        if !self.clear_init_seen.insert(name.clone()) {
            return Ok(());
        }
        let loop_var = self.fresh_loop_var("lDelay");
        let upper = {
            let mut b = FirBuilder::new(self.store);
            b.int32(i32::try_from(size).map_err(|_| {
                SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("delay line size conversion overflow: {size}"),
                )
            })?)
        };
        let zero = match self.types.get(&sig) {
            Some(SimpleSigType::Int) => {
                let mut b = FirBuilder::new(self.store);
                b.int32(0)
            }
            Some(SimpleSigType::Real) => {
                let mut b = FirBuilder::new(self.store);
                match self.real_ty {
                    FirType::Float64 => b.float64(0.0),
                    _ => b.float32(0.0),
                }
            }
            _ => {
                return Err(SignalFirError::new(
                    SignalFirErrorCode::UnsupportedSignalNode,
                    format!("cannot zero-init delay-line for signal {}", sig.as_u32()),
                ));
            }
        };
        let body = {
            let index = {
                let mut b = FirBuilder::new(self.store);
                b.load_var(loop_var.clone(), AccessType::Loop, FirType::Int32)
            };
            let store_node = {
                let mut b = FirBuilder::new(self.store);
                b.store_table(name, AccessType::Struct, index, zero)
            };
            let mut b = FirBuilder::new(self.store);
            b.block(&[store_node])
        };
        let mut b = FirBuilder::new(self.store);
        self.clear_statements
            .push(b.simple_for_loop(loop_var, upper, body, false));
        Ok(())
    }
}

// ─── DelayLoweringCtx ────────────────────────────────────────────────────────

/// Borrow bundle for strategy-local FIR emission during lowering.
pub(crate) struct DelayLoweringCtx<'a> {
    pub(crate) store: &'a mut FirStore,
    pub(crate) immediate_statements: &'a mut Vec<FirId>,
    pub(crate) post_output_statements: &'a mut Vec<FirId>,
    pub(crate) next_loop_var_id: &'a mut usize,
}
