//! Table-generator sub-module production (`--table-init runtime`).
//!
//! Port of the C++ `signal2Container` / `generateSigGen` path: a `SIGGEN`
//! payload is compiled into its own FIR program whose `fill` function computes
//! the table content at initialization time, instead of being evaluated at
//! compile time by [`crate::signal_fir::siggen`].
//!
//! # Why a separate lowering
//!
//! The generator is an ordinary 0-input / 1-output deterministic DSP. It has
//! its own state (recursion carriers, delay lines), its own sample-rate
//! constants, and possibly its own tables. Compiling it through the same
//! `build_module` pipeline as the main program — with the output sink pointed
//! at the `table` argument — is what makes sample-rate-dependent and
//! foreign-function content expressible at all: those values simply do not
//! exist at compile time.
//!
//! # Nesting
//!
//! A generator that reads another generated table owns that table's sub-module
//! in turn, and its fill must run first. This is contract C5 of
//! `porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`, and it
//! is deliberately **not** upstream behavior: Faust 2.87.1 declares the inner
//! table of a nested generator but never fills it, leaving it zero
//! (`porting/generated/siggen-table-init-s0/`, fixture `f08`).

use fir::{FirId, FirType};
use signals::{SigId, SigMatch, match_sig};
use ui::UiProgram;

use super::build::FillSpec;
use super::{SignalFirError, SignalFirErrorCode, SignalToFirLower};

/// One generator compiled into a sub-module, ready to be referenced by the
/// enclosing program.
pub(super) struct GeneratedTableFiller {
    /// Sub-module class name, `{module}SIG{k}`.
    pub(super) name: String,
    /// The imported `SubModule` node, already interned in the parent store.
    pub(super) node: FirId,
}

impl SignalToFirLower<'_> {
    /// Compiles one `SIGGEN` payload into a sub-module of the current program.
    ///
    /// `size` is the table length, used only by the caller to emit the `fill`
    /// call; the sub-module itself is length-agnostic and loops over its
    /// `count` argument, exactly as the C++ `fill` method does.
    pub(super) fn build_generator_sub_module(
        &mut self,
        generator: SigId,
        elem_ty: &FirType,
    ) -> Result<GeneratedTableFiller, SignalFirError> {
        let payload = match match_sig(self.arena, generator) {
            SigMatch::Gen(inner) => inner,
            _ => generator,
        };
        let name = self.next_sub_module_name();

        // The generator is prepared exactly like the main program, so the
        // interpreter path and this path see the same normalized shape. This
        // is the same call `siggen::interpret_generator` already makes.
        let prepared = crate::signal_prepare::prepare_signals_for_fir_verified(
            self.arena,
            &[payload],
            &UiProgram::empty(),
        )
        .map_err(|err| {
            SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!("table generator preparation failed: {err}"),
            )
        })?;

        let outputs = prepared.outputs();
        if outputs.len() != 1 {
            return Err(SignalFirError::new(
                SignalFirErrorCode::UnsupportedSignalNode,
                format!(
                    "table generator must have exactly one output, got {}",
                    outputs.len()
                ),
            ));
        }

        let plan = super::super::planner::SignalFirPlan {
            num_inputs: 0,
            num_outputs: 1,
            signal_count: outputs.len(),
        };
        let spec = FillSpec {
            name: name.clone(),
            elem_ty: elem_ty.clone(),
        };
        let empty_ui = UiProgram::empty();
        let lowered = super::build::build_module(
            &plan,
            &name,
            prepared.arena(),
            outputs,
            &empty_ui,
            prepared.types_map(),
            prepared.sig_types_map(),
            prepared.origins(),
            self.real_ty(),
            self.delay.options().max_copy_delay,
            self.delay.options().delay_line_threshold,
            super::super::ComputeMode::Scalar,
            super::super::ControlRateMode::InlinePerBlock,
            super::super::ProcessingApi::Block,
            self.table_init_mode,
            None,
            None,
            Some(&spec),
        )?;

        // `FirId`s are store-local: the generator was lowered into its own
        // store and must be re-interned here before the parent module can
        // reference it.
        let node = self.store.import_from(&lowered.store, lowered.module);
        Ok(GeneratedTableFiller { name, node })
    }

    /// Allocates the next sub-module name, `{module}SIG{k}` (C++
    /// `getFreshID(getClassName() + "SIG")`).
    fn next_sub_module_name(&mut self) -> String {
        let k = self.name_gen.sub_module_counter;
        self.name_gen.sub_module_counter += 1;
        format!("{}SIG{k}", self.module_name)
    }
}
