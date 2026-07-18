//! `cranelift` backend (early bring-up).
//!
//! # Role
//! - Planned native-code backend lowering Faust FIR to machine code via
//!   Cranelift, with a companion `cranelift_dsp` C/C++ export layer.
//!
//! # C++ provenance note
//! - There is no direct C++ `cranelift` backend in upstream Faust.
//! - This backend is a Rust-native extension and follows parity requirements
//!   documented in `porting/cranelift-backend-plan-en.md` for exported runtime
//!   behavior (`llvm_dsp` / `interpreter_dsp`-style API strategy).
//!
//! # Current status
//! - Early backend bring-up with a real Cranelift JIT integration:
//!   - a finalized `compute` symbol is emitted,
//!   - finalized code is kept alive by an owned `JITModule`,
//!   - a backend `dsp*` layout contract is derived from FIR `globals`.
//! - FIR `compute` body lowering is implemented incrementally through a
//!   supported subset (loops, arithmetic, selected control flow, part of math
//!   intrinsics, struct globals/tables, etc.).
//! - When the FIR body exceeds the current subset, the backend deliberately
//!   falls back to a valid no-op `compute` stub instead of failing the whole
//!   compilation.
//!
//! # Design notes (current phase)
//! - The backend prioritizes compile-path integration and diagnosability over
//!   runtime parity completeness.
//! - `FAUSTFLOAT` maps to `f32` by default, or `f64` when
//!   `CraneliftOptions::double_precision` is set (`-double`).
//! - The exported FFI/runtime layer (`cranelift_dsp`) can consume diagnostic
//!   metadata such as whether `compute` was really lowered or stubbed.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    AbiParam, FuncRef, InstBuilder, MemFlags, StackSlotData, StackSlotKind, Type, Value, types,
};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, Init, Linkage, Module, default_libcall_names};
use fir::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, match_fir};
use std::collections::HashMap;
use std::ffi::c_void;

mod api;
mod core;
mod host;
mod jit_data;
mod lowering;
mod subset;

pub use api::{diagnose_cranelift_compute_subset_gap, generate_cranelift_module};
pub use core::{
    BACKEND_NAME, CraneliftBackendError, CraneliftBackendErrorCode, CraneliftOptLevel,
    CraneliftOptions, JitDspModule, StructFieldKind, StructFieldLayout, StructLayoutPlan,
    backend_id,
};

pub(crate) use core::*;
pub(crate) use host::*;
pub(crate) use jit_data::*;
pub(crate) use lowering::*;
pub(crate) use subset::*;

#[cfg(test)]
mod tests;
