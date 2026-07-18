//! Backend modules colocated under `codegen`.
//!
//! # Organization
//! - Implemented backends:
//!   - [`c`], [`cpp`]
//! - Shared helpers:
//!   - internal `faust_api` module (DSP API signature validation)
//!   - internal `c_family` module (syntax-parameterless emission shared by
//!     `c`/`cpp`; see `porting/c-family-emitter-core-plan-2026-07-04-en.md`)
//! - Scaffolded backends (planned parity targets):
//!   - `cranelift`, `cmajor`, `codebox`, `csharp`, `dlang`, `interp`, `jax`, `jsfx`,
//!     `julia`, `llvm`, `rust`, `sdf3`, `vhdl`, `wasm`.
//!
//! # Module contract
//! - Each backend module owns:
//!   - option struct(s),
//!   - typed backend error surface,
//!   - generation entry point(s) from FIR module roots.
//! - Unsupported FIR nodes must fail with stable backend-specific error codes.
//!
//! # API mapping status
//! - Implemented backends expose `adapted` APIs (parity-driven behavior with
//!   Rust-native options/results).

pub(crate) mod c_family;
pub(crate) mod faust_api;
pub(crate) mod purity;

pub mod asc;
pub mod c;
pub mod cmajor;
pub mod codebox;
pub mod cpp;
#[cfg(not(target_arch = "wasm32"))]
pub mod cranelift;
pub mod csharp;
pub mod dlang;
pub mod interp;
pub mod jax;
pub mod jsfx;
pub mod julia;
pub mod llvm;
pub mod rust;
pub mod sdf3;
pub mod vhdl;
pub mod wasm;
