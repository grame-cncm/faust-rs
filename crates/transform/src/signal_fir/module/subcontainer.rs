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
use signals::SigId;

use super::{SignalFirError, SignalToFirLower};

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
        let name = self.next_sub_module_name();
        let spec = super::subcontainer_compile::GeneratorSubModuleSpec {
            name: &name,
            elem_ty: elem_ty.clone(),
            real_ty: self.real_ty(),
            max_copy_delay: self.delay.options().max_copy_delay,
            delay_line_threshold: self.delay.options().delay_line_threshold,
            table_init_mode: self.table_init_mode,
            table_init_sample_rate: self.table_init_sample_rate,
            check_table: self.check_table,
            scheduling_strategy: self.scheduling_strategy,
        };
        let node = super::subcontainer_compile::compile_generator_sub_module(
            self.arena,
            &mut self.store,
            generator,
            &spec,
        )?;
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
