//! Backend modules colocated under `codegen`.
//!
//! # Organization
//! - Implemented backends:
//!   - [`c`], [`codebox`], [`cpp`]
//! - Shared helpers:
//!   - internal `faust_api` module (DSP API signature validation)
//!   - internal `c_family` module (syntax-parameterless emission shared by
//!     `c`/`cpp`; see `porting/c-family-emitter-core-plan-2026-07-04-en.md`)
//!   - internal `textual` module (precedence-aware expression layout reusable
//!     by textual targets)
//! - Scaffolded backends (planned parity targets):
//!   - `cranelift`, `cmajor`, `csharp`, `dlang`, `interp`, `jax`, `jsfx`,
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
pub(crate) mod textual;

use fir::{FirId, FirMatch, FirStore, match_fir};

/// Returns the names of the sub-modules a module carries, in declaration order.
///
/// A sub-module is a table generator whose `fill` function computes a table's
/// content at initialization time
/// (`porting/siggen-subcontainer-table-init-port-plan-2026-08-05-en.md`).
/// Every backend must consult this before emitting: a backend that has not been
/// migrated to emit sub-modules has to fail, because the table declaration it
/// *does* emit would then be filled by nothing and read as zeros. Silence here
/// is a wrong-answer bug, not a missing feature.
///
/// Empty for every program without a generated table, which is the common case
/// and costs one decode.
pub(crate) fn sub_module_names(store: &FirStore, sub_modules: FirId) -> Vec<String> {
    let FirMatch::Block(items) = match_fir(store, sub_modules) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::SubModule { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

/// FIR function names a backend must never emit as an ordinary function.
///
/// Each of these is part of the DSP lifecycle and is rendered into the
/// backend's own surface: `staticInit` becomes the body of `classInit`
/// (`dspsetup` in codebox), `compute` becomes the target's compute entry point,
/// and so on. A backend that walks its `functions` block and emits whatever it
/// does not recognize will emit these a second time — producing a duplicate
/// definition that, for `staticInit`, references locals that only exist inside
/// `classInit`.
///
/// That mistake was made independently in the `c`, `rust` and `julia` backends
/// before this list existed. `is_lifecycle_function` is the single place to
/// consult; `backends::emits_no_lifecycle_leak` in the compiler test suite is
/// what catches a backend that forgets to.
const LIFECYCLE_FUNCTIONS: &[&str] = &[
    "staticInit",
    "metadata",
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "buildUserInterface",
    "compute",
    "control",
    "frame",
];

/// Returns `true` when `name` is a lifecycle function the backend renders into
/// its own surface rather than emitting verbatim.
#[must_use]
pub fn is_lifecycle_function(name: &str) -> bool {
    LIFECYCLE_FUNCTIONS.contains(&name)
}

/// Message body for a backend's "sub-modules not supported yet" rejection, so
/// the diagnostics stay uniform while each backend keeps its own stable error
/// code.
///
/// **No caller as of plan phase S5** (2026-08-06): every backend emits
/// generated-table sub-modules, so a sub-module reaching a backend is an
/// internal error rather than an unsupported feature, and each backend says so
/// in its own words. This is kept for the next backend to be added, which will
/// need exactly this refusal between the day it can decode a module and the day
/// it can fill a table — the alternative being to emit a table that nothing
/// writes, which reads as zeros.
pub fn unsupported_sub_modules_message(backend: &str, names: &[String]) -> String {
    format!(
        "the `{backend}` backend cannot yet emit generated-table sub-modules ({}); \
         compile with `--table-init const` to fold the table at compile time instead",
        names.join(", ")
    )
}

pub mod codegen_error;

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
