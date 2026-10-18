//! Statement scheduling of `compute` for the JIT backends (interp, Cranelift,
//! wasm): W3 store forwarding, then W2 read sinking, then W7 local reuse.
//!
//! Design: `porting/wasm-w2-w3-sinking-forwarding-design-2026-10-17-en.md`.
//! Plan and measurements: `porting/wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`
//! (C2, C3, W2, W3).
//!
//! - **W3** (`forward`): after `fRec0[0] = fRecBody6;`, a later load of
//!   `fRec0[0]` in the same straight-line region reads `fRecBody6`.
//! - **W2** (`sink`): a pure declaration such as `double fTemp36 = fRec2[1];` is
//!   emitted just before its first reader, or before the first statement that
//!   may write what it reads.
//! - **W7** (`coalesce`): stack scalars whose lifetimes do not overlap share
//!   one variable declared at the top of `compute`, so that a wasm engine that
//!   gives each local a stack slot (JavaScriptCore) keeps a small frame
//!   (`porting/fir-local-reuse-w7-design-2026-10-17-en.md`).
//!
//! All three keep the arithmetic and the values: outputs are bit-identical.
//! Each rewrite is accepted only when its independent checker (`check` for
//! W3/W2, `check_reuse` for W7) re-derives it; otherwise `compute` keeps its previous form (fail closed, and an
//! assertion in debug builds).
//!
//! # Public API mapping status
//! - `schedule_compute`: no C++ counterpart (`adapted`, Rust-only). Faust C++
//!   emits FIR order; LLVM, which compiles its C++ output, sinks and forwards on
//!   its own, which the JIT backends here do not (TurboFan keeps wasm loads in
//!   program order).

mod check;
mod check_reuse;
mod coalesce;
mod effects;
mod forward;
mod sink;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_reuse;

use std::collections::HashMap;

use crate::{FirBuilder, FirId, FirMatch, FirStore, match_fir};

/// The `compute` declaration of a module's `functions` block: its position, its
/// name, type, arguments, body and inline flag.
fn find_compute(store: &FirStore, functions: FirId) -> Option<(usize, FirId)> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return None;
    };
    items
        .iter()
        .enumerate()
        .find_map(|(at, &id)| match match_fir(store, id) {
            FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } if name == "compute" && matches!(match_fir(store, body), FirMatch::Block(_)) => {
                Some((at, id))
            }
            _ => None,
        })
}

/// Forwards, sinks, then reuses the locals of the body `body` of `compute` in
/// `store`; returns the new body (`body` itself when nothing changes; a stage
/// its checker rejects is dropped).
fn schedule_body(store: &mut FirStore, body: FirId) -> FirId {
    let mut writes = HashMap::new();
    forward::local_write_counts(store, body, &mut writes);
    let forwarded = {
        let mut forwarder = forward::Forwarder::new(store, &writes);
        forwarder.block(body, &mut forward::Available::default())
    };
    let forwarded = match check::check_forward(store, body, forwarded) {
        Ok(()) => forwarded,
        Err(reason) => {
            debug_assert!(false, "W3 forwarding rejected by its checker: {reason}");
            body
        }
    };
    let (sunk, orders) = {
        let mut sinker = sink::Sinker::new(store, sink::SLACK);
        let sunk = sinker.block(forwarded);
        (sunk, sinker.orders)
    };
    let sunk = match check::check_sink(store, forwarded, sunk, &orders) {
        Ok(()) => sunk,
        Err(reason) => {
            debug_assert!(false, "W2 sinking rejected by its checker: {reason}");
            forwarded
        }
    };
    let Some(reused) = coalesce::reuse_locals(store, sunk) else {
        return sunk;
    };
    match check_reuse::check_reuse(store, sunk, reused) {
        Ok(()) => reused,
        Err(reason) => {
            debug_assert!(false, "W7 local reuse rejected by its checker: {reason}");
            sunk
        }
    }
}

/// Schedules `compute` (W3 forwarding, W2 sinking, then W7 local reuse) in a
/// copy of `module`.
///
/// Returns the new store and module root, or `None` when `module` has no
/// `compute` body or nothing changes. Used by the interp, Cranelift and wasm
/// backends; the other backends keep FIR order.
#[must_use]
pub fn schedule_compute(src: &FirStore, module: FirId) -> Option<(FirStore, FirId)> {
    let FirMatch::Module { functions, .. } = match_fir(src, module) else {
        return None;
    };
    find_compute(src, functions)?;

    let mut store = FirStore::new();
    let root = store.import_from(src, module);
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(&store, root)
    else {
        return None;
    };
    let (at, compute) = find_compute(&store, functions)?;
    let FirMatch::DeclareFun {
        name: fun_name,
        typ,
        args,
        body: Some(body),
        is_inline,
    } = match_fir(&store, compute)
    else {
        return None;
    };
    let new_body = schedule_body(&mut store, body);
    if new_body == body {
        return None;
    }
    let FirMatch::Block(mut items) = match_fir(&store, functions) else {
        return None;
    };
    let sub_modules = match match_fir(&store, sub_modules) {
        FirMatch::Block(subs) => subs,
        _ => Vec::new(),
    };
    let mut b = FirBuilder::new(&mut store);
    items[at] = b.declare_fun(fun_name, typ, &args, Some(new_body), is_inline);
    let functions = b.block(&items);
    let root = b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &sub_modules,
    );
    Some((store, root))
}
