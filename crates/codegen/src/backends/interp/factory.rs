//! `FbcDspFactory<R>` — compiled FBC bytecode program ready for instantiation.
//!
//! # Source provenance (C++)
//! - `interpreter_dsp_factory_aux<REAL, TRACE>` in `interpreter_dsp_aux.hh`
//!
//! # Design notes
//! - Holds 6 bytecode blocks (as [`BlockId`]s into the [`FbcBlockArena`]),
//!   plus metadata and UI instruction lists.
//! - The `optimize()` method applies the bytecode optimizer (levels 1..opt_level)
//!   to all 6 code blocks, guarded by a one-shot flag.
//! - No `TRACE` template parameter — tracing is a future runtime option.

use super::bytecode::{BlockId, FbcBlockArena, FbcMetaInstruction, FbcUiInstruction};
use super::optimizer::optimize_block;
use super::real::FbcReal;

/// Compiled FBC program ready for instantiation.
///
/// # Source provenance (C++)
/// - `interpreter_dsp_factory_aux<REAL, TRACE>` in `interpreter_dsp_aux.hh`
///
/// Generic over `R` (the REAL type: `f32` or `f64`).
///
/// # Fields
/// - `name`, `sha_key`, `compile_options`: factory identity/metadata.
/// - `version`: `.fbc` file format version (must match [`INTERP_FILE_VERSION`]).
/// - `num_inputs`, `num_outputs`: audio channel counts.
/// - `int_heap_size`, `real_heap_size`: heap allocation sizes for instances.
/// - `sr_offset`, `count_offset`, `iota_offset`: well-known heap slot indices.
/// - `opt_level`: maximum optimizer level (0 = no optimization).
/// - `arena`: block arena owning all bytecode blocks.
/// - `meta_block`, `ui_block`: metadata and UI instruction lists.
/// - 6 `BlockId`s referencing code blocks in the arena.
///
/// The factory is intentionally immutable after construction except for the
/// optional one-shot optimizer pass. Runtime heaps and DSP execution state live
/// in `FbcDspInstance`, not here.
///
/// [`INTERP_FILE_VERSION`]: super::opcode::INTERP_FILE_VERSION
/// The run-time layout of a batch factory (`generate_interp_batch_module`):
/// the voice count, the slot of `fVoices` the instance writes it into, and the
/// slot of the `voice` argument it writes before each per-voice call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbcBatchLayout {
    pub voices: i32,
    pub voices_offset: i32,
    pub voice_offset: i32,
}

#[derive(Debug)]
pub struct FbcDspFactory<R: FbcReal> {
    // ── Identity / metadata ────────────────────────────────────────────
    pub name: String,
    pub sha_key: String,
    pub compile_options: String,
    pub version: u32,

    // ── Audio layout ───────────────────────────────────────────────────
    pub num_inputs: i32,
    pub num_outputs: i32,

    // ── Memory layout ──────────────────────────────────────────────────
    pub int_heap_size: i32,
    pub real_heap_size: i32,
    pub sr_offset: i32,
    pub count_offset: i32,
    pub iota_offset: i32,

    // ── Optimizer ──────────────────────────────────────────────────────
    pub opt_level: i32,
    optimized: bool,

    // ── Block arena ────────────────────────────────────────────────────
    pub arena: FbcBlockArena<R>,

    // ── Data blocks ────────────────────────────────────────────────────
    pub meta_block: Vec<FbcMetaInstruction>,
    pub ui_block: Vec<FbcUiInstruction<R>>,

    // ── Code block IDs ─────────────────────────────────────────────────
    pub static_init_block: BlockId,
    pub init_block: BlockId,
    pub reset_ui_block: BlockId,
    pub clear_block: BlockId,
    pub compute_block: BlockId,
    pub compute_dsp_block: BlockId,

    // ── Batch ──────────────────────────────────────────────────────────
    /// `Some` for a factory of a `fir::batch` module; not written to `.fbc`.
    pub batch: Option<FbcBatchLayout>,
}

impl<R: FbcReal> FbcDspFactory<R> {
    /// Creates a new factory with all fields provided.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_factory_aux` constructor in `interpreter_dsp_aux.hh`
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        sha_key: impl Into<String>,
        compile_options: impl Into<String>,
        version: u32,
        num_inputs: i32,
        num_outputs: i32,
        int_heap_size: i32,
        real_heap_size: i32,
        sr_offset: i32,
        count_offset: i32,
        iota_offset: i32,
        opt_level: i32,
        arena: FbcBlockArena<R>,
        meta_block: Vec<FbcMetaInstruction>,
        ui_block: Vec<FbcUiInstruction<R>>,
        static_init_block: BlockId,
        init_block: BlockId,
        reset_ui_block: BlockId,
        clear_block: BlockId,
        compute_block: BlockId,
        compute_dsp_block: BlockId,
    ) -> Self {
        Self {
            name: name.into(),
            sha_key: sha_key.into(),
            compile_options: compile_options.into(),
            version,
            num_inputs,
            num_outputs,
            int_heap_size,
            real_heap_size,
            sr_offset,
            count_offset,
            iota_offset,
            opt_level,
            optimized: false,
            arena,
            meta_block,
            ui_block,
            static_init_block,
            init_block,
            reset_ui_block,
            clear_block,
            compute_block,
            compute_dsp_block,
            batch: None,
        }
    }

    /// Applies bytecode optimization (levels 1..opt_level) to all 6 code blocks.
    ///
    /// This is idempotent: the first call optimizes, subsequent calls are no-ops.
    /// Optimization is factory-wide on purpose so every instance created from
    /// the same factory sees the same stabilized bytecode layout.
    ///
    /// # Source provenance (C++)
    /// - `interpreter_dsp_factory_aux::optimize()` in `interpreter_dsp.hh`
    ///   (only runs when `TRACE == 0` and `!fOptimized`).
    pub fn optimize(&mut self) {
        if self.optimized {
            return;
        }
        self.optimized = true;

        if self.opt_level <= 0 {
            return;
        }

        let max = self.opt_level as u8;
        optimize_block::<R>(&mut self.arena, self.static_init_block, 1, max);
        optimize_block::<R>(&mut self.arena, self.init_block, 1, max);
        optimize_block::<R>(&mut self.arena, self.reset_ui_block, 1, max);
        optimize_block::<R>(&mut self.arena, self.clear_block, 1, max);
        optimize_block::<R>(&mut self.arena, self.compute_block, 1, max);
        optimize_block::<R>(&mut self.arena, self.compute_dsp_block, 1, max);
    }

    /// Returns whether this factory has already been optimized.
    #[must_use]
    pub fn is_optimized(&self) -> bool {
        self.optimized
    }

    /// Returns the number of soundfile slots required by this factory.
    ///
    /// Derived by counting `AddSoundfile` opcodes in the UI instruction list.
    /// Used by [`super::instance::FbcDspInstance`] to pre-populate executor
    /// soundfile slots with default silence.
    #[must_use]
    pub fn soundfile_count(&self) -> usize {
        use super::opcode::FbcOpcode;
        self.ui_block
            .iter()
            .filter(|i| i.opcode == FbcOpcode::AddSoundfile)
            .count()
    }
}

#[cfg(test)]
mod tests;
