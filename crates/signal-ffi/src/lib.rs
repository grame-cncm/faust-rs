//! `signal-ffi` — C/C++ export layer for Faust signal construction/matching.
//!
//! # Overview
//! This crate owns the `Csig*` (constructors) and `CisSig*` (matchers) symbols,
//! plus the normalization (`CsimplifyToNormalForm`) and source-generation
//! (`CcreateSourceFromSignals`) helpers, and a thin C++ wrapper header
//! (`libfaust-signal.h`).
//!
//! The implementation is backed by:
//! - [`signals::SigBuilder`] / [`signals::match_sig`] for signal
//!   construction/matching,
//! - [`tlib::TreeArena`] for hash-consed node storage,
//! - `normalize` for normal-form simplification.
//!
//! # Context model
//! Signal handles share the process-global [`tree_ffi::FfiTreeContext`] with
//! `box-ffi` (`createLibContext` / `destroyLibContext`), so `Box` and `Signal`
//! handles can be printed and freed through the common libfaust helpers
//! (`CprintSignal`, `freeCMemory`) without a separate lifecycle.
//!
//! # Safety model
//! All exported functions follow C ABI contracts. Caller pointers are validated
//! at entry points; invalid handles produce null/false/0 results, and matcher
//! out-parameters are written only on a successful match.
//!
//! # Mapping status
//! The exported symbol names intentionally mirror the C++ reference header
//! `architecture/faust/dsp/libfaust-signal-c.h` (Faust commit `8eebea429`).
//! This is an incremental parity layer: rows marked as exact candidates in the
//! generated Signal API matrix still need focused semantic parity tests.

#![allow(unsafe_code)]
#![allow(non_snake_case)] // FFI parity requires preserving C API symbol names.

use std::ffi::{CStr, c_char, c_int, c_void};

use signals::{BinOp, SigBuilder, SigMatch, match_sig};
use tlib::{NodeKind, TreeId, de_bruijn_aperture, de_bruijn_ref, vec_to_list};
use tree_ffi::{
    FfiSignalControlKind, FfiTreeContext, SOperator, SType, with_global_context as with_ctx,
    write_out_handle as unsafe_write_out_signal, write_out_int as unsafe_write_out_int,
    write_out_real as unsafe_write_out_real,
};

fn null_signal() -> *mut c_void {
    std::ptr::null_mut()
}

fn decode_signal(ctx: &FfiTreeContext, signal: *mut c_void) -> Option<TreeId> {
    ctx.decode(signal)
}

fn encode_signal(ctx: &mut FfiTreeContext, signal: TreeId) -> *mut c_void {
    ctx.encode(signal)
}

unsafe fn decode_signal_array(
    ctx: &FfiTreeContext,
    signals: *mut *mut c_void,
) -> Option<Vec<TreeId>> {
    if signals.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut cur = signals;
    loop {
        // SAFETY: caller provides a valid null-terminated signal handle array.
        let handle = unsafe { *cur };
        if handle.is_null() {
            break;
        }
        out.push(decode_signal(ctx, handle)?);
        // SAFETY: same null-terminated array contract as the dereference above.
        cur = unsafe { cur.add(1) };
    }
    Some(out)
}

unsafe fn decode_label(ctx: &mut FfiTreeContext, label: *const c_char) -> Option<TreeId> {
    // SAFETY: caller provides a null or valid NUL-terminated label pointer.
    unsafe { ctx.label_tree(label) }
}

unsafe fn read_c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller provides a valid NUL-terminated C string.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

unsafe fn read_c_string_array(ptrs: *const *const c_char) -> Option<Vec<String>> {
    if ptrs.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut cur = ptrs;
    loop {
        // SAFETY: caller provides a valid null-terminated C string pointer array.
        let ptr = unsafe { *cur };
        if ptr.is_null() {
            break;
        }
        // SAFETY: each non-null array element must be a valid NUL-terminated C string.
        out.push(unsafe { read_c_string(ptr) }?);
        // SAFETY: same null-terminated array contract as the dereference above.
        cur = unsafe { cur.add(1) };
    }
    Some(out)
}

unsafe fn read_stype_array(types: *const SType) -> Option<Vec<SType>> {
    if types.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut cur = types;
    loop {
        // SAFETY: caller provides a valid kSInt/kSReal array terminated by kSInt
        // (the C++ API uses the zero enum value as terminator).
        let value = unsafe { std::ptr::read(cur) };
        if matches!(value, SType::kSInt) {
            break;
        }
        out.push(value);
        // SAFETY: same zero-terminated array contract as the read above.
        cur = unsafe { cur.add(1) };
    }
    Some(out)
}

fn stype_tree(ctx: &mut FfiTreeContext, ty: SType) -> TreeId {
    match ty {
        SType::kSInt => ctx.arena.int(0),
        SType::kSReal => ctx.arena.int(1),
    }
}

fn string_list(ctx: &mut FfiTreeContext, values: &[String]) -> TreeId {
    let mut list = ctx.arena.nil();
    for value in values.iter().rev() {
        let item = ctx.arena.symbol(value);
        list = ctx.arena.cons(item, list);
    }
    list
}

fn stype_list(ctx: &mut FfiTreeContext, values: &[SType]) -> TreeId {
    let mut list = ctx.arena.nil();
    for value in values.iter().rev() {
        let item = stype_tree(ctx, *value);
        list = ctx.arena.cons(item, list);
    }
    list
}

fn ffunction_descriptor(
    ctx: &mut FfiTreeContext,
    rtype: SType,
    names: &[String],
    atypes: &[SType],
    incfile: &str,
    libfile: &str,
) -> TreeId {
    let ret_type = stype_tree(ctx, rtype);
    let names = string_list(ctx, names);
    let atypes = stype_list(ctx, atypes);
    let payload = ctx.arena.cons(names, atypes);
    let signature = ctx.arena.cons(ret_type, payload);
    let incfile = ctx.arena.symbol(incfile);
    let libfile = ctx.arena.symbol(libfile);
    let tag = ctx.arena.intern_tag("FFUN");
    ctx.arena
        .intern(NodeKind::Tag(tag), &[signature, incfile, libfile])
}

fn write_out_signal(ctx: &mut FfiTreeContext, out: *mut *mut c_void, value: TreeId) {
    // SAFETY: exported predicate functions receive optional out-pointers from C.
    unsafe { unsafe_write_out_signal(ctx, out, value) }
}

fn write_out_int(out: *mut c_int, value: i32) {
    // SAFETY: exported predicate functions receive optional out-pointers from C.
    unsafe { unsafe_write_out_int(out, value) }
}

fn write_out_real(out: *mut f64, value: f64) {
    // SAFETY: exported predicate functions receive optional out-pointers from C.
    unsafe { unsafe_write_out_real(out, value) }
}

fn write_out_i64(out: *mut i64, value: i64) {
    if !out.is_null() {
        // SAFETY: exported predicate functions receive optional out-pointers from C.
        unsafe {
            *out = value;
        }
    }
}

fn control_label(ctx: &FfiTreeContext, control: u32) -> Option<TreeId> {
    ctx.signal_control(control).map(|control| control.label)
}

fn control_slider_fields(
    ctx: &FfiTreeContext,
    control: u32,
) -> Option<(TreeId, TreeId, TreeId, TreeId, TreeId)> {
    let control = ctx.signal_control(control)?;
    Some((
        control.label,
        control.init?,
        control.min?,
        control.max?,
        control.step?,
    ))
}

fn control_bargraph_fields(ctx: &FfiTreeContext, control: u32) -> Option<(TreeId, TreeId, TreeId)> {
    let control = ctx.signal_control(control)?;
    Some((control.label, control.min?, control.max?))
}

fn soperator_to_binop(op: SOperator) -> BinOp {
    match op {
        SOperator::kAdd => BinOp::Add,
        SOperator::kSub => BinOp::Sub,
        SOperator::kMul => BinOp::Mul,
        SOperator::kDiv => BinOp::Div,
        SOperator::kRem => BinOp::Rem,
        SOperator::kLsh => BinOp::Lsh,
        SOperator::kARsh => BinOp::ARsh,
        SOperator::kLRsh => BinOp::LRsh,
        SOperator::kGT => BinOp::Gt,
        SOperator::kLT => BinOp::Lt,
        SOperator::kGE => BinOp::Ge,
        SOperator::kLE => BinOp::Le,
        SOperator::kEQ => BinOp::Eq,
        SOperator::kNE => BinOp::Ne,
        SOperator::kAND => BinOp::And,
        SOperator::kOR => BinOp::Or,
        SOperator::kXOR => BinOp::Xor,
    }
}

fn binop_to_raw(op: BinOp) -> c_int {
    c_int::try_from(op as i64).unwrap_or_default()
}

fn unary_signal(
    input: *mut c_void,
    build: impl FnOnce(&mut SigBuilder<'_>, TreeId) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(input) = decode_signal(ctx, input) else {
            return null_signal();
        };
        let output = build(&mut SigBuilder::new(&mut ctx.arena), input);
        encode_signal(ctx, output)
    })
}

fn binary_signal(
    left: *mut c_void,
    right: *mut c_void,
    build: impl FnOnce(&mut SigBuilder<'_>, TreeId, TreeId) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(left), Some(right)) = (decode_signal(ctx, left), decode_signal(ctx, right))
        else {
            return null_signal();
        };
        let output = build(&mut SigBuilder::new(&mut ctx.arena), left, right);
        encode_signal(ctx, output)
    })
}

fn ternary_signal(
    first: *mut c_void,
    second: *mut c_void,
    third: *mut c_void,
    build: impl FnOnce(&mut SigBuilder<'_>, TreeId, TreeId, TreeId) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(first), Some(second), Some(third)) = (
            decode_signal(ctx, first),
            decode_signal(ctx, second),
            decode_signal(ctx, third),
        ) else {
            return null_signal();
        };
        let output = build(&mut SigBuilder::new(&mut ctx.arena), first, second, third);
        encode_signal(ctx, output)
    })
}

fn quaternary_signal(
    first: *mut c_void,
    second: *mut c_void,
    third: *mut c_void,
    fourth: *mut c_void,
    build: impl FnOnce(&mut SigBuilder<'_>, TreeId, TreeId, TreeId, TreeId) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(first), Some(second), Some(third), Some(fourth)) = (
            decode_signal(ctx, first),
            decode_signal(ctx, second),
            decode_signal(ctx, third),
            decode_signal(ctx, fourth),
        ) else {
            return null_signal();
        };
        let output = build(
            &mut SigBuilder::new(&mut ctx.arena),
            first,
            second,
            third,
            fourth,
        );
        encode_signal(ctx, output)
    })
}

fn slider_signal(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
    kind: FfiSignalControlKind,
    build: impl FnOnce(&mut SigBuilder<'_>, u32) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: C API label pointers are optional NUL-terminated strings.
        let Some(label) = (unsafe { decode_label(ctx, label) }) else {
            return null_signal();
        };
        let (Some(init), Some(min), Some(max), Some(step)) = (
            decode_signal(ctx, init),
            decode_signal(ctx, min),
            decode_signal(ctx, max),
            decode_signal(ctx, step),
        ) else {
            return null_signal();
        };
        let control =
            ctx.register_signal_control(kind, label, Some(init), Some(min), Some(max), Some(step));
        let output = build(&mut SigBuilder::new(&mut ctx.arena), control);
        encode_signal(ctx, output)
    })
}

fn bargraph_signal(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
    signal: *mut c_void,
    kind: FfiSignalControlKind,
    build: impl FnOnce(&mut SigBuilder<'_>, u32, TreeId) -> TreeId,
) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: C API label pointers are optional NUL-terminated strings.
        let Some(label) = (unsafe { decode_label(ctx, label) }) else {
            return null_signal();
        };
        let (Some(min), Some(max), Some(signal)) = (
            decode_signal(ctx, min),
            decode_signal(ctx, max),
            decode_signal(ctx, signal),
        ) else {
            return null_signal();
        };
        let control = ctx.register_signal_control(kind, label, None, Some(min), Some(max), None);
        let output = build(&mut SigBuilder::new(&mut ctx.arena), control, signal);
        encode_signal(ctx, output)
    })
}

macro_rules! binary_export {
    ($name:ident, $builder:ident) => {
        #[unsafe(no_mangle)]
        /// Builds one binary Signal node.
        pub extern "C" fn $name(x: *mut c_void, y: *mut c_void) -> *mut c_void {
            binary_signal(x, y, |b, x, y| b.$builder(x, y))
        }
    };
}

macro_rules! unary_export {
    ($name:ident, $builder:ident) => {
        #[unsafe(no_mangle)]
        /// Builds one unary Signal node.
        pub extern "C" fn $name(x: *mut c_void) -> *mut c_void {
            unary_signal(x, |b, x| b.$builder(x))
        }
    };
}

#[unsafe(no_mangle)]
/// Builds an integer Signal constant.
pub extern "C" fn CsigInt(n: c_int) -> *mut c_void {
    with_ctx(|ctx| {
        let output = SigBuilder::new(&mut ctx.arena).int(n);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds a 64-bit integer Signal constant.
pub extern "C" fn CsigInt64(n: i64) -> *mut c_void {
    with_ctx(|ctx| {
        let output = ctx.arena.int(n);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds a real Signal constant.
pub extern "C" fn CsigReal(n: f64) -> *mut c_void {
    with_ctx(|ctx| {
        let output = SigBuilder::new(&mut ctx.arena).real(n);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one input Signal.
pub extern "C" fn CsigInput(idx: c_int) -> *mut c_void {
    if idx < 0 {
        return null_signal();
    }
    with_ctx(|ctx| {
        let output = SigBuilder::new(&mut ctx.arena).input(idx);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one arbitrary binary operator Signal.
pub extern "C" fn CsigBinOp(op: SOperator, x: *mut c_void, y: *mut c_void) -> *mut c_void {
    binary_signal(x, y, |b, x, y| b.binop(soperator_to_binop(op), x, y))
}

binary_export!(CsigAdd, add);
binary_export!(CsigSub, sub);
binary_export!(CsigMul, mul);
binary_export!(CsigDiv, div);
binary_export!(CsigRem, rem);
binary_export!(CsigLeftShift, lsh);
binary_export!(CsigLRightShift, lrsh);
binary_export!(CsigARightShift, arsh);
binary_export!(CsigGT, gt);
binary_export!(CsigLT, lt);
binary_export!(CsigGE, ge);
binary_export!(CsigLE, le);
binary_export!(CsigEQ, eq);
binary_export!(CsigNE, ne);
binary_export!(CsigAND, and);
binary_export!(CsigOR, or);
binary_export!(CsigXOR, xor);
binary_export!(CsigRemainder, remainder);
binary_export!(CsigPow, pow);
binary_export!(CsigMin, min);
binary_export!(CsigMax, max);
binary_export!(CsigFmod, fmod);
binary_export!(CsigAtan2, atan2);
binary_export!(CsigAttach, attach);

unary_export!(CsigAbs, abs);
unary_export!(CsigAcos, acos);
unary_export!(CsigTan, tan);
unary_export!(CsigSqrt, sqrt);
unary_export!(CsigSin, sin);
unary_export!(CsigRint, rint);
unary_export!(CsigLog, log);
unary_export!(CsigLog10, log10);
unary_export!(CsigFloor, floor);
unary_export!(CsigExp, exp);
unary_export!(CsigExp10, exp10);
unary_export!(CsigCos, cos);
unary_export!(CsigCeil, ceil);
unary_export!(CsigAtan, atan);
unary_export!(CsigAsin, asin);
unary_export!(CsigDelay1, delay1);
unary_export!(CsigIntCast, int_cast);
unary_export!(CsigFloatCast, float_cast);

#[unsafe(no_mangle)]
/// Builds one explicit delay Signal.
pub extern "C" fn CsigDelay(s: *mut c_void, del: *mut c_void) -> *mut c_void {
    binary_signal(s, del, |b, s, del| b.delay(s, del))
}

#[unsafe(no_mangle)]
/// Builds the single-output recursive self reference used inside `CsigRecursion`.
pub extern "C" fn CsigSelf() -> *mut c_void {
    CsigSelfN(0)
}

#[unsafe(no_mangle)]
/// Builds one recursive Signal around a single body expression.
pub extern "C" fn CsigRecursion(s: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(body) = decode_signal(ctx, s) else {
            return null_signal();
        };
        let body_list = vec_to_list(&mut ctx.arena, &[body]);
        let rec = SigBuilder::new(&mut ctx.arena).rec(body_list);
        let output = SigBuilder::new(&mut ctx.arena).proj(0, rec);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one indexed recursive self reference used inside `CsigRecursionN`.
pub extern "C" fn CsigSelfN(id: c_int) -> *mut c_void {
    if id < 0 {
        return null_signal();
    }
    with_ctx(|ctx| {
        let rec_ref = de_bruijn_ref(&mut ctx.arena, 1);
        let projection = SigBuilder::new(&mut ctx.arena).proj(id, rec_ref);
        let output = SigBuilder::new(&mut ctx.arena).delay1(projection);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one recursive Signal block from a null-terminated Signal array.
///
/// # Safety
/// `rf` must be null or point to a valid null-terminated array of Signal
/// handles created by this shared FFI context.
pub unsafe extern "C" fn CsigRecursionN(rf: *mut *mut c_void) -> *mut *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: caller provides a valid null-terminated signal handle array.
        let Some(inputs) = (unsafe { decode_signal_array(ctx, rf) }) else {
            return std::ptr::null_mut();
        };
        let rec_body = vec_to_list(&mut ctx.arena, &inputs);
        let rec = SigBuilder::new(&mut ctx.arena).rec(rec_body);
        let outputs = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                if de_bruijn_aperture(&ctx.arena, *input) > 0 {
                    let slot = c_int::try_from(index).unwrap_or_default();
                    let projection = SigBuilder::new(&mut ctx.arena).proj(slot, rec);
                    encode_signal(ctx, projection)
                } else {
                    encode_signal(ctx, *input)
                }
            })
            .collect();
        ctx.alloc_handle_ptr_array(outputs)
    })
}

#[unsafe(no_mangle)]
/// Builds one read-only table Signal.
pub extern "C" fn CsigReadOnlyTable(
    n: *mut c_void,
    init: *mut c_void,
    ridx: *mut c_void,
) -> *mut c_void {
    ternary_signal(n, init, ridx, |b, n, init, ridx| {
        b.read_only_table(n, init, ridx)
    })
}

#[unsafe(no_mangle)]
/// Builds one read/write table Signal.
pub extern "C" fn CsigWriteReadTable(
    n: *mut c_void,
    init: *mut c_void,
    widx: *mut c_void,
    wsig: *mut c_void,
    ridx: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(n), Some(init), Some(widx), Some(wsig), Some(ridx)) = (
            decode_signal(ctx, n),
            decode_signal(ctx, init),
            decode_signal(ctx, widx),
            decode_signal(ctx, wsig),
            decode_signal(ctx, ridx),
        ) else {
            return null_signal();
        };
        let output = SigBuilder::new(&mut ctx.arena).write_read_table(n, init, widx, wsig, ridx);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one waveform Signal from a null-terminated Signal array.
///
/// # Safety
/// `wf` must be null or point to a valid null-terminated array of Signal
/// handles created by this shared FFI context.
pub unsafe extern "C" fn CsigWaveform(wf: *mut *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: caller provides a valid null-terminated signal handle array.
        let Some(values) = (unsafe { decode_signal_array(ctx, wf) }) else {
            return null_signal();
        };
        let output = SigBuilder::new(&mut ctx.arena).waveform(&values);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one soundfile Signal.
///
/// # Safety
/// `label` must be null or point to a valid NUL-terminated C string.
pub unsafe extern "C" fn CsigSoundfile(label: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: C API label pointers are optional NUL-terminated strings.
        let Some(label) = (unsafe { decode_label(ctx, label) }) else {
            return null_signal();
        };
        let control = ctx.register_signal_control(
            FfiSignalControlKind::Soundfile,
            label,
            None,
            None,
            None,
            None,
        );
        let output = SigBuilder::new(&mut ctx.arena).soundfile(control);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one soundfile length Signal.
pub extern "C" fn CsigSoundfileLength(sf: *mut c_void, part: *mut c_void) -> *mut c_void {
    binary_signal(sf, part, |b, sf, part| b.soundfile_length(sf, part))
}

#[unsafe(no_mangle)]
/// Builds one soundfile rate Signal.
pub extern "C" fn CsigSoundfileRate(sf: *mut c_void, part: *mut c_void) -> *mut c_void {
    binary_signal(sf, part, |b, sf, part| b.soundfile_rate(sf, part))
}

#[unsafe(no_mangle)]
/// Builds one soundfile buffer Signal.
pub extern "C" fn CsigSoundfileBuffer(
    sf: *mut c_void,
    chan: *mut c_void,
    part: *mut c_void,
    ridx: *mut c_void,
) -> *mut c_void {
    quaternary_signal(sf, chan, part, ridx, |b, sf, chan, part, ridx| {
        b.soundfile_buffer(sf, chan, part, ridx)
    })
}

#[unsafe(no_mangle)]
/// Builds one two-way selector Signal.
pub extern "C" fn CsigSelect2(
    selector: *mut c_void,
    s1: *mut c_void,
    s2: *mut c_void,
) -> *mut c_void {
    ternary_signal(selector, s1, s2, |b, selector, s1, s2| {
        b.select2(selector, s1, s2)
    })
}

#[unsafe(no_mangle)]
/// Builds one three-way selector Signal.
pub extern "C" fn CsigSelect3(
    selector: *mut c_void,
    s1: *mut c_void,
    s2: *mut c_void,
    s3: *mut c_void,
) -> *mut c_void {
    quaternary_signal(selector, s1, s2, s3, |b, selector, s1, s2, s3| {
        b.select3(selector, s1, s2, s3)
    })
}

#[unsafe(no_mangle)]
/// Builds one foreign-function Signal.
///
/// The `atypes` array follows the C++ API contract and is terminated by
/// `kSInt` (`0`), while `names` and `largs` are null-terminated arrays.
///
/// # Safety
/// `names`, `atypes`, `incfile`, `libfile`, and `largs` must be valid according
/// to their null/zero-terminated C API contracts.
pub unsafe extern "C" fn CsigFFun(
    rtype: SType,
    names: *const *const c_char,
    atypes: *const SType,
    incfile: *const c_char,
    libfile: *const c_char,
    largs: *mut *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: exported C API receives null/zero-terminated arrays.
        let Some(names) = (unsafe { read_c_string_array(names) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives a zero-terminated SType array.
        let Some(atypes) = (unsafe { read_stype_array(atypes) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(incfile) = (unsafe { read_c_string(incfile) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(libfile) = (unsafe { read_c_string(libfile) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives a null-terminated signal array.
        let Some(largs) = (unsafe { decode_signal_array(ctx, largs) }) else {
            return null_signal();
        };

        let ff = ffunction_descriptor(ctx, rtype, &names, &atypes, &incfile, &libfile);
        let args = vec_to_list(&mut ctx.arena, &largs);
        let output = SigBuilder::new(&mut ctx.arena).ffun(ff, args);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one foreign-constant Signal.
///
/// # Safety
/// `name` and `file` must be valid NUL-terminated C strings.
pub unsafe extern "C" fn CsigFConst(
    ty: SType,
    name: *const c_char,
    file: *const c_char,
) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(name) = (unsafe { read_c_string(name) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(file) = (unsafe { read_c_string(file) }) else {
            return null_signal();
        };
        let ty = stype_tree(ctx, ty);
        let name = ctx.arena.symbol(name);
        let file = ctx.arena.symbol(file);
        let output = SigBuilder::new(&mut ctx.arena).fconst(ty, name, file);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one foreign-variable Signal.
///
/// # Safety
/// `name` and `file` must be valid NUL-terminated C strings.
pub unsafe extern "C" fn CsigFVar(
    ty: SType,
    name: *const c_char,
    file: *const c_char,
) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(name) = (unsafe { read_c_string(name) }) else {
            return null_signal();
        };
        // SAFETY: exported C API receives valid NUL-terminated strings.
        let Some(file) = (unsafe { read_c_string(file) }) else {
            return null_signal();
        };
        let ty = stype_tree(ctx, ty);
        let name = ctx.arena.symbol(name);
        let file = ctx.arena.symbol(file);
        let output = SigBuilder::new(&mut ctx.arena).fvar(ty, name, file);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Simplifies one Signal to Rust's current normal-form preparation subset.
pub extern "C" fn CsimplifyToNormalForm(s: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(signal) = decode_signal(ctx, s) else {
            return null_signal();
        };
        let ui = box_ffi::signal_only_root_ui(ctx, "FaustDSP");
        let opts = normalize::normalform::NormalFormOpts::default();
        match normalize::normalform::prepare_signals(&mut ctx.arena, &ui, signal, &opts) {
            Ok((normal, _types)) => encode_signal(ctx, normal),
            Err(_e) => null_signal(),
        }
    })
}

#[unsafe(no_mangle)]
/// Simplifies a null-terminated Signal array to Rust's current normal-form
/// preparation subset.
///
/// # Safety
/// `siglist` must point to a valid null-terminated Signal handle array.
pub unsafe extern "C" fn CsimplifyToNormalForm2(siglist: *mut *mut c_void) -> *mut *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: exported C API receives a null-terminated signal array.
        let Some(signals) = (unsafe { decode_signal_array(ctx, siglist) }) else {
            return std::ptr::null_mut();
        };
        if signals.is_empty() {
            return ctx.alloc_handle_ptr_array(Vec::new());
        }
        let ui = box_ffi::signal_only_root_ui(ctx, "FaustDSP");
        let opts = normalize::normalform::NormalFormOpts::default();
        match normalize::normalform::prepare_signals_multi(&mut ctx.arena, &ui, &signals, &opts) {
            Ok((normal, _types)) => {
                let raw = normal
                    .into_iter()
                    .map(|signal| ctx.encode(signal))
                    .collect();
                ctx.alloc_handle_ptr_array(raw)
            }
            Err(_e) => std::ptr::null_mut(),
        }
    })
}

#[unsafe(no_mangle)]
/// Compile a null-terminated Signal array to target source code.
///
/// # Safety
/// - `name_app` must be null or a valid NUL-terminated C string.
/// - `osigs` must point to a valid null-terminated Signal handle array.
/// - `lang` must be a valid NUL-terminated C string.
/// - `argv` must be null or point to `argc` valid C string pointers.
/// - `error_msg` must be null or point to a writable 4096-byte buffer.
pub unsafe extern "C" fn CcreateSourceFromSignals(
    name_app: *const c_char,
    osigs: *mut *mut c_void,
    lang: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    let name_app = match unsafe { ffi_common::optional_c_str_arg(name_app, "name_app") } {
        Ok(Some(s)) if !s.is_empty() => s.to_owned(),
        Ok(_) => "FaustDSP".to_owned(),
        Err(e) => {
            unsafe { ffi_common::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let lang = match unsafe { ffi_common::required_c_str_arg(lang, "lang") } {
        Ok(s) => s.to_ascii_lowercase(),
        Err(e) => {
            unsafe { ffi_common::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let argv = match unsafe { ffi_common::decode_c_argv(argc, argv) } {
        Ok(v) => v,
        Err(e) => {
            unsafe { ffi_common::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    // SAFETY: caller upholds the null-terminated Signal array contract.
    match unsafe {
        box_ffi::export_source_from_signal_array_handle(
            &name_app,
            osigs.cast::<c_void>(),
            &lang,
            &argv,
        )
    } {
        Ok(source) => ffi_common::alloc_c_string(&source),
        Err(e) => {
            unsafe { ffi_common::write_error_4096(error_msg, &e) };
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
/// Builds one button Signal.
///
/// # Safety
/// `label` must be null or point to a valid NUL-terminated C string.
pub unsafe extern "C" fn CsigButton(label: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: C API label pointers are optional NUL-terminated strings.
        let Some(label) = (unsafe { decode_label(ctx, label) }) else {
            return null_signal();
        };
        let control = ctx.register_signal_control(
            FfiSignalControlKind::Button,
            label,
            None,
            None,
            None,
            None,
        );
        let output = SigBuilder::new(&mut ctx.arena).button(control);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one checkbox Signal.
///
/// # Safety
/// `label` must be null or point to a valid NUL-terminated C string.
pub unsafe extern "C" fn CsigCheckbox(label: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        // SAFETY: C API label pointers are optional NUL-terminated strings.
        let Some(label) = (unsafe { decode_label(ctx, label) }) else {
            return null_signal();
        };
        let control = ctx.register_signal_control(
            FfiSignalControlKind::Checkbox,
            label,
            None,
            None,
            None,
            None,
        );
        let output = SigBuilder::new(&mut ctx.arena).checkbox(control);
        encode_signal(ctx, output)
    })
}

#[unsafe(no_mangle)]
/// Builds one vertical slider Signal.
pub extern "C" fn CsigVSlider(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    slider_signal(
        label,
        init,
        min,
        max,
        step,
        FfiSignalControlKind::VSlider,
        |b, control| b.vslider(control),
    )
}

#[unsafe(no_mangle)]
/// Builds one horizontal slider Signal.
pub extern "C" fn CsigHSlider(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    slider_signal(
        label,
        init,
        min,
        max,
        step,
        FfiSignalControlKind::HSlider,
        |b, control| b.hslider(control),
    )
}

#[unsafe(no_mangle)]
/// Builds one numeric-entry Signal.
pub extern "C" fn CsigNumEntry(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    slider_signal(
        label,
        init,
        min,
        max,
        step,
        FfiSignalControlKind::NumEntry,
        |b, control| b.numentry(control),
    )
}

#[unsafe(no_mangle)]
/// Builds one vertical bargraph Signal.
pub extern "C" fn CsigVBargraph(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
    s: *mut c_void,
) -> *mut c_void {
    bargraph_signal(
        label,
        min,
        max,
        s,
        FfiSignalControlKind::VBargraph,
        |b, control, s| b.vbargraph(control, s),
    )
}

#[unsafe(no_mangle)]
/// Builds one horizontal bargraph Signal.
pub extern "C" fn CsigHBargraph(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
    s: *mut c_void,
) -> *mut c_void {
    bargraph_signal(
        label,
        min,
        max,
        s,
        FfiSignalControlKind::HBargraph,
        |b, control, s| b.hbargraph(control, s),
    )
}

#[unsafe(no_mangle)]
/// Matches an integer Signal constant.
pub extern "C" fn CisSigInt(t: *mut c_void, i: *mut c_int) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Int(value) => {
                write_out_int(i, value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a real Signal constant.
pub extern "C" fn CisSigReal(t: *mut c_void, r: *mut f64) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Real(value) => {
                write_out_real(r, value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches an input Signal.
pub extern "C" fn CisSigInput(t: *mut c_void, i: *mut c_int) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Input(value) => {
                write_out_int(i, value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a one-sample delay Signal.
pub extern "C" fn CisSigDelay1(t: *mut c_void, t0: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Delay1(value) => {
                write_out_signal(ctx, t0, value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches an explicit delay Signal.
pub extern "C" fn CisSigDelay(t: *mut c_void, t0: *mut *mut c_void, t1: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Delay(left, right) => {
                write_out_signal(ctx, t0, left);
                write_out_signal(ctx, t1, right);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a binary operator Signal.
pub extern "C" fn CisSigBinOp(
    s: *mut c_void,
    op: *mut c_int,
    x: *mut *mut c_void,
    y: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::BinOp(binop, left, right) => {
                write_out_int(op, binop_to_raw(binop));
                write_out_signal(ctx, x, left);
                write_out_signal(ctx, y, right);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a 64-bit integer Signal constant.
pub extern "C" fn CisSigInt64(t: *mut c_void, i: *mut i64) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match ctx.arena.kind(t) {
            Some(NodeKind::Int(value)) => {
                write_out_i64(i, *value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches an output Signal.
pub extern "C" fn CisSigOutput(t: *mut c_void, i: *mut c_int, t0: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Output(index, signal) => {
                write_out_int(i, index);
                write_out_signal(ctx, t0, signal);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a prefix Signal.
pub extern "C" fn CisSigPrefix(t: *mut c_void, t0: *mut *mut c_void, t1: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Prefix(left, right) => {
                write_out_signal(ctx, t0, left);
                write_out_signal(ctx, t1, right);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a read-table Signal.
pub extern "C" fn CisSigRDTbl(s: *mut c_void, t: *mut *mut c_void, i: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::RdTbl(table, index) => {
                write_out_signal(ctx, t, table);
                write_out_signal(ctx, i, index);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a write-table Signal.
pub extern "C" fn CisSigWRTbl(
    u: *mut c_void,
    id: *mut *mut c_void,
    t: *mut *mut c_void,
    i: *mut *mut c_void,
    s: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(u) = decode_signal(ctx, u) else {
            return false;
        };
        match match_sig(&ctx.arena, u) {
            SigMatch::WrTbl(size, generator, index, signal) => {
                write_out_signal(ctx, id, size);
                write_out_signal(ctx, t, generator);
                write_out_signal(ctx, i, index);
                write_out_signal(ctx, s, signal);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a generator Signal.
pub extern "C" fn CisSigGen(t: *mut c_void, x: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Gen(value) => {
                write_out_signal(ctx, x, value);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a generator Signal without returning its child.
pub extern "C" fn CisSigGen1(t: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        matches!(match_sig(&ctx.arena, t), SigMatch::Gen(_))
    })
}

#[unsafe(no_mangle)]
/// Matches a two-way selector Signal.
pub extern "C" fn CisSigSelect2(
    t: *mut c_void,
    selector: *mut *mut c_void,
    s1: *mut *mut c_void,
    s2: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::Select2(sel, left, right) => {
                write_out_signal(ctx, selector, sel);
                write_out_signal(ctx, s1, left);
                write_out_signal(ctx, s2, right);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches an assert-bounds Signal.
pub extern "C" fn CisSigAssertBounds(
    t: *mut c_void,
    s1: *mut *mut c_void,
    s2: *mut *mut c_void,
    s3: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(t) = decode_signal(ctx, t) else {
            return false;
        };
        match match_sig(&ctx.arena, t) {
            SigMatch::AssertBounds(a, b, c) => {
                write_out_signal(ctx, s1, a);
                write_out_signal(ctx, s2, b);
                write_out_signal(ctx, s3, c);
                true
            }
            _ => false,
        }
    })
}

macro_rules! match_unary_predicate {
    ($name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        /// Matches one unary Signal family.
        pub extern "C" fn $name(s: *mut c_void, x: *mut *mut c_void) -> bool {
            with_ctx(|ctx| {
                let Some(s) = decode_signal(ctx, s) else {
                    return false;
                };
                match match_sig(&ctx.arena, s) {
                    $variant(value) => {
                        write_out_signal(ctx, x, value);
                        true
                    }
                    _ => false,
                }
            })
        }
    };
}

match_unary_predicate!(CisSigHighest, SigMatch::Highest);
match_unary_predicate!(CisSigLowest, SigMatch::Lowest);
match_unary_predicate!(CisSigIntCast, SigMatch::IntCast);
match_unary_predicate!(CisSigFloatCast, SigMatch::FloatCast);

#[unsafe(no_mangle)]
/// Matches a foreign-function Signal.
pub extern "C" fn CisSigFFun(
    s: *mut c_void,
    ff: *mut *mut c_void,
    largs: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::FFun(fun, args) => {
                write_out_signal(ctx, ff, fun);
                write_out_signal(ctx, largs, args);
                true
            }
            _ => false,
        }
    })
}

macro_rules! match_ternary_predicate {
    ($name:ident, $variant:path, $a:ident, $b:ident, $c:ident) => {
        #[unsafe(no_mangle)]
        /// Matches one ternary Signal family.
        pub extern "C" fn $name(
            s: *mut c_void,
            $a: *mut *mut c_void,
            $b: *mut *mut c_void,
            $c: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(s) = decode_signal(ctx, s) else {
                    return false;
                };
                match match_sig(&ctx.arena, s) {
                    $variant(first, second, third) => {
                        write_out_signal(ctx, $a, first);
                        write_out_signal(ctx, $b, second);
                        write_out_signal(ctx, $c, third);
                        true
                    }
                    _ => false,
                }
            })
        }
    };
}

match_ternary_predicate!(CisSigFConst, SigMatch::FConst, r#type, name, file);
match_ternary_predicate!(CisSigFVar, SigMatch::FVar, r#type, name, file);

#[unsafe(no_mangle)]
/// Matches a projection Signal.
pub extern "C" fn CisProj(s: *mut c_void, i: *mut c_int, rgroup: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::Proj(index, group) => {
                write_out_int(i, index);
                write_out_signal(ctx, rgroup, group);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a recursive Signal group.
pub extern "C" fn CisRec(s: *mut c_void, var: *mut *mut c_void, body: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::Rec(rec_body) => {
                let nil = ctx.arena.nil();
                write_out_signal(ctx, var, nil);
                write_out_signal(ctx, body, rec_body);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a button Signal.
pub extern "C" fn CisSigButton(s: *mut c_void, lbl: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::Button(control) => match control_label(ctx, control) {
                Some(label) => {
                    write_out_signal(ctx, lbl, label);
                    true
                }
                None => false,
            },
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a checkbox Signal.
pub extern "C" fn CisSigCheckbox(s: *mut c_void, lbl: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::Checkbox(control) => match control_label(ctx, control) {
                Some(label) => {
                    write_out_signal(ctx, lbl, label);
                    true
                }
                None => false,
            },
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a waveform Signal.
pub extern "C" fn CisSigWaveform(s: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        matches!(match_sig(&ctx.arena, s), SigMatch::Waveform(_))
    })
}

macro_rules! match_slider_predicate {
    ($name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        /// Matches one slider-like Signal family.
        pub extern "C" fn $name(
            s: *mut c_void,
            lbl: *mut *mut c_void,
            init: *mut *mut c_void,
            min: *mut *mut c_void,
            max: *mut *mut c_void,
            step: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(s) = decode_signal(ctx, s) else {
                    return false;
                };
                match match_sig(&ctx.arena, s) {
                    $variant(control) => match control_slider_fields(ctx, control) {
                        Some((label, init_v, min_v, max_v, step_v)) => {
                            write_out_signal(ctx, lbl, label);
                            write_out_signal(ctx, init, init_v);
                            write_out_signal(ctx, min, min_v);
                            write_out_signal(ctx, max, max_v);
                            write_out_signal(ctx, step, step_v);
                            true
                        }
                        None => false,
                    },
                    _ => false,
                }
            })
        }
    };
}

match_slider_predicate!(CisSigHSlider, SigMatch::HSlider);
match_slider_predicate!(CisSigVSlider, SigMatch::VSlider);
match_slider_predicate!(CisSigNumEntry, SigMatch::NumEntry);

macro_rules! match_bargraph_predicate {
    ($name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        /// Matches one bargraph Signal family.
        pub extern "C" fn $name(
            s: *mut c_void,
            lbl: *mut *mut c_void,
            min: *mut *mut c_void,
            max: *mut *mut c_void,
            x: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(s) = decode_signal(ctx, s) else {
                    return false;
                };
                match match_sig(&ctx.arena, s) {
                    $variant(control, signal) => match control_bargraph_fields(ctx, control) {
                        Some((label, min_v, max_v)) => {
                            write_out_signal(ctx, lbl, label);
                            write_out_signal(ctx, min, min_v);
                            write_out_signal(ctx, max, max_v);
                            write_out_signal(ctx, x, signal);
                            true
                        }
                        None => false,
                    },
                    _ => false,
                }
            })
        }
    };
}

match_bargraph_predicate!(CisSigHBargraph, SigMatch::HBargraph);
match_bargraph_predicate!(CisSigVBargraph, SigMatch::VBargraph);

macro_rules! match_binary_predicate {
    ($name:ident, $variant:path, $left:ident, $right:ident) => {
        #[unsafe(no_mangle)]
        /// Matches one binary Signal family.
        pub extern "C" fn $name(
            s: *mut c_void,
            $left: *mut *mut c_void,
            $right: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(s) = decode_signal(ctx, s) else {
                    return false;
                };
                match match_sig(&ctx.arena, s) {
                    $variant(first, second) => {
                        write_out_signal(ctx, $left, first);
                        write_out_signal(ctx, $right, second);
                        true
                    }
                    _ => false,
                }
            })
        }
    };
}

match_binary_predicate!(CisSigAttach, SigMatch::Attach, s0, s1);
match_binary_predicate!(CisSigEnable, SigMatch::Enable, s0, s1);
match_binary_predicate!(CisSigControl, SigMatch::Control, s0, s1);
match_binary_predicate!(CisSigSoundfileLength, SigMatch::SoundfileLength, sf, part);
match_binary_predicate!(CisSigSoundfileRate, SigMatch::SoundfileRate, sf, part);

#[unsafe(no_mangle)]
/// Matches a soundfile Signal.
pub extern "C" fn CisSigSoundfile(s: *mut c_void, label: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::Soundfile(control) => match control_label(ctx, control) {
                Some(lbl) => {
                    write_out_signal(ctx, label, lbl);
                    true
                }
                None => false,
            },
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Matches a soundfile buffer Signal.
pub extern "C" fn CisSigSoundfileBuffer(
    s: *mut c_void,
    sf: *mut *mut c_void,
    chan: *mut *mut c_void,
    part: *mut *mut c_void,
    ridx: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(s) = decode_signal(ctx, s) else {
            return false;
        };
        match match_sig(&ctx.arena, s) {
            SigMatch::SoundfileBuffer(sf_v, chan_v, part_v, ridx_v) => {
                write_out_signal(ctx, sf, sf_v);
                write_out_signal(ctx, chan, chan_v);
                write_out_signal(ctx, part, part_v);
                write_out_signal(ctx, ridx, ridx_v);
                true
            }
            _ => false,
        }
    })
}

#[unsafe(no_mangle)]
/// Deterministic false predicate for unsupported doc constant table Signals.
pub extern "C" fn CisSigDocConstantTbl(
    _t: *mut c_void,
    _n: *mut *mut c_void,
    _sig: *mut *mut c_void,
) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Deterministic false predicate for unsupported doc write table Signals.
pub extern "C" fn CisSigDocWriteTbl(
    _t: *mut c_void,
    _n: *mut *mut c_void,
    _sig: *mut *mut c_void,
    _widx: *mut *mut c_void,
    _wsig: *mut *mut c_void,
) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Deterministic false predicate for unsupported doc access table Signals.
pub extern "C" fn CisSigDocAccessTbl(
    _t: *mut c_void,
    _tbl: *mut *mut c_void,
    _ridx: *mut *mut c_void,
) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::ptr;
    use std::sync::{Mutex, MutexGuard};
    use tree_ffi::{FfiSignalControlKind, reset_global_context, with_global_context};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_context() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().expect("test mutex poisoned")
    }

    unsafe fn print_signal(signal: *mut c_void) -> String {
        let ptr = box_ffi::CprintSignal(signal, false, 4096);
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { box_ffi::freeCMemory(ptr.cast()) };
        text
    }

    #[test]
    fn builds_basic_signals_and_matches_them() {
        let _guard = lock_context();
        reset_global_context();

        let int_sig = CsigInt(7);
        assert!(!int_sig.is_null());
        let mut int_out = 0;
        assert!(CisSigInt(int_sig, &mut int_out));
        assert_eq!(int_out, 7);

        let real_sig = CsigReal(0.5);
        let mut real_out = 0.0;
        assert!(CisSigReal(real_sig, &mut real_out));
        assert_eq!(real_out, 0.5);

        let input_sig = CsigInput(0);
        let mut input_out = -1;
        assert!(CisSigInput(input_sig, &mut input_out));
        assert_eq!(input_out, 0);

        let add_sig = CsigAdd(int_sig, input_sig);
        let mut op = -1;
        let mut lhs = ptr::null_mut();
        let mut rhs = ptr::null_mut();
        assert!(CisSigBinOp(add_sig, &mut op, &mut lhs, &mut rhs));
        assert_eq!(op, SOperator::kAdd as c_int);
        assert_eq!(lhs, int_sig);
        assert_eq!(rhs, input_sig);

        let delay1 = CsigDelay1(input_sig);
        let mut delayed = ptr::null_mut();
        assert!(CisSigDelay1(delay1, &mut delayed));
        assert_eq!(delayed, input_sig);

        let delay = CsigDelay(input_sig, CsigInt(2));
        let mut signal_out = ptr::null_mut();
        let mut amount_out = ptr::null_mut();
        assert!(CisSigDelay(delay, &mut signal_out, &mut amount_out));
        assert_eq!(signal_out, input_sig);
        assert!(CisSigInt(amount_out, &mut int_out));
        assert_eq!(int_out, 2);

        let rendered = unsafe { print_signal(CsigExp10(input_sig)) };
        assert!(rendered.contains("SIGEXP10"));
    }

    #[test]
    fn rejects_null_and_invalid_signal_inputs() {
        let _guard = lock_context();
        reset_global_context();

        assert!(CsigInput(-1).is_null());
        assert!(CsigAdd(ptr::null_mut(), CsigInt(1)).is_null());
        assert!(!CisSigInt(ptr::null_mut(), ptr::null_mut()));
        assert!(!CisSigBinOp(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut()
        ));
    }

    #[test]
    fn builds_table_waveform_soundfile_and_ui_signals() {
        let _guard = lock_context();
        reset_global_context();

        let zero = CsigInt(0);
        let one = CsigInt(1);
        let size = CsigInt(2);
        let input = CsigInput(0);

        let table = CsigReadOnlyTable(size, input, zero);
        assert!(unsafe { print_signal(table) }.contains("SIGRDTBL"));

        let rw_table = CsigWriteReadTable(size, input, zero, one, zero);
        assert!(unsafe { print_signal(rw_table) }.contains("SIGRDTBL"));

        let mut waveform_values = [zero, one, ptr::null_mut()];
        let waveform = unsafe { CsigWaveform(waveform_values.as_mut_ptr()) };
        assert!(unsafe { print_signal(waveform) }.contains("SIGWAVEFORM"));
        assert!(CisSigWaveform(waveform));

        let label = CString::new("drum[url:{'kick.wav'}]").expect("valid label");
        let soundfile = unsafe { CsigSoundfile(label.as_ptr()) };
        assert!(!soundfile.is_null());
        let mut label_out = ptr::null_mut();
        assert!(CisSigSoundfile(soundfile, &mut label_out));
        assert!(!label_out.is_null());
        let length = CsigSoundfileLength(soundfile, zero);
        assert!(!length.is_null());
        let mut sf_out = ptr::null_mut();
        let mut part_out = ptr::null_mut();
        assert!(CisSigSoundfileLength(length, &mut sf_out, &mut part_out));
        assert_eq!(sf_out, soundfile);
        assert_eq!(part_out, zero);
        assert!(!CsigSoundfileRate(soundfile, zero).is_null());
        assert!(!CsigSoundfileBuffer(soundfile, zero, zero, input).is_null());

        let gain = CString::new("gain[style:knob]").expect("valid label");
        let slider = CsigHSlider(gain.as_ptr(), zero, zero, one, CsigReal(0.01));
        assert!(!slider.is_null());
        let mut init_out = ptr::null_mut();
        let mut min_out = ptr::null_mut();
        let mut max_out = ptr::null_mut();
        let mut step_out = ptr::null_mut();
        assert!(CisSigHSlider(
            slider,
            &mut label_out,
            &mut init_out,
            &mut min_out,
            &mut max_out,
            &mut step_out
        ));
        assert_eq!(init_out, zero);
        assert_eq!(min_out, zero);
        assert_eq!(max_out, one);
        let button = unsafe { CsigButton(gain.as_ptr()) };
        assert!(!button.is_null());
        assert!(CisSigButton(button, &mut label_out));
        let attach = CsigAttach(slider, button);
        assert!(!attach.is_null());
        let mut lhs = ptr::null_mut();
        let mut rhs = ptr::null_mut();
        assert!(CisSigAttach(attach, &mut lhs, &mut rhs));
        assert_eq!(lhs, slider);
        assert_eq!(rhs, button);

        with_global_context(|ctx| {
            assert!(
                ctx.signal_controls()
                    .iter()
                    .any(|control| control.kind == FfiSignalControlKind::Soundfile)
            );
            assert!(
                ctx.signal_controls()
                    .iter()
                    .any(|control| control.kind == FfiSignalControlKind::HSlider)
            );
            assert!(
                ctx.signal_controls()
                    .iter()
                    .any(|control| control.kind == FfiSignalControlKind::Button)
            );
        });
    }

    #[test]
    fn builds_recursive_signal_handles() {
        let _guard = lock_context();
        reset_global_context();

        let input = CsigInput(0);
        let self_ref = CsigSelf();
        assert!(unsafe { print_signal(self_ref) }.contains("DEBRUIJNREF"));

        let recursive = CsigRecursion(CsigAdd(input, self_ref));
        let rendered = unsafe { print_signal(recursive) };
        assert!(rendered.contains("SIGPROJ"));
        assert!(rendered.contains("SIGREC"));
        let mut rec_index = -1;
        let mut rec_group = ptr::null_mut();
        assert!(CisProj(recursive, &mut rec_index, &mut rec_group));
        assert_eq!(rec_index, 0);
        let mut rec_var = ptr::null_mut();
        let mut rec_body = ptr::null_mut();
        assert!(CisRec(rec_group, &mut rec_var, &mut rec_body));
        assert!(!rec_var.is_null());
        assert!(!rec_body.is_null());

        let closed = CsigReal(3.0);
        let mut block = [CsigAdd(input, CsigSelfN(0)), closed, ptr::null_mut()];
        let outputs = unsafe { CsigRecursionN(block.as_mut_ptr()) };
        assert!(!outputs.is_null());
        // SAFETY: `CsigRecursionN` returns a context-owned null-terminated handle array.
        let first = unsafe { *outputs };
        // SAFETY: second element exists because the input array had two elements.
        let second = unsafe { *outputs.add(1) };
        // SAFETY: third element is the terminating null pointer.
        let terminator = unsafe { *outputs.add(2) };
        assert!(unsafe { print_signal(first) }.contains("SIGPROJ"));
        let mut real_out = 0.0;
        assert!(CisSigReal(second, &mut real_out));
        assert_eq!(real_out, 3.0);
        assert!(terminator.is_null());
        unsafe { box_ffi::freeCMemory(outputs.cast()) };
    }

    #[test]
    fn builds_and_matches_foreign_signal_nodes() {
        let _guard = lock_context();
        reset_global_context();

        let name = CString::new("fSamplingFreq").expect("valid name");
        let file = CString::new("faust/dsp/dsp.h").expect("valid file");
        let fconst = unsafe { CsigFConst(SType::kSReal, name.as_ptr(), file.as_ptr()) };
        assert!(!fconst.is_null());
        let mut ty_out = ptr::null_mut();
        let mut name_out = ptr::null_mut();
        let mut file_out = ptr::null_mut();
        assert!(CisSigFConst(
            fconst,
            &mut ty_out,
            &mut name_out,
            &mut file_out
        ));
        assert!(!ty_out.is_null());
        assert!(!name_out.is_null());
        assert!(!file_out.is_null());

        let var_name = CString::new("count").expect("valid name");
        let fvar = unsafe { CsigFVar(SType::kSInt, var_name.as_ptr(), file.as_ptr()) };
        assert!(!fvar.is_null());
        assert!(CisSigFVar(fvar, &mut ty_out, &mut name_out, &mut file_out));

        let fun_name = CString::new("tanh").expect("valid function name");
        let incfile = CString::new("math.h").expect("valid include");
        let libfile = CString::new("").expect("valid library");
        let names = [fun_name.as_ptr(), ptr::null()];
        let atypes = [SType::kSReal, SType::kSInt];
        let arg = CsigInput(0);
        let mut args = [arg, ptr::null_mut()];
        let ffun = unsafe {
            CsigFFun(
                SType::kSReal,
                names.as_ptr(),
                atypes.as_ptr(),
                incfile.as_ptr(),
                libfile.as_ptr(),
                args.as_mut_ptr(),
            )
        };
        assert!(!ffun.is_null());
        let mut ff_out = ptr::null_mut();
        let mut args_out = ptr::null_mut();
        assert!(CisSigFFun(ffun, &mut ff_out, &mut args_out));
        assert!(!ff_out.is_null());
        assert!(!args_out.is_null());
    }

    #[test]
    fn simplifies_signal_handles_and_arrays() {
        let _guard = lock_context();
        reset_global_context();

        let mixed = CsigAdd(CsigInt(1), CsigReal(0.5));
        let normal = CsimplifyToNormalForm(mixed);
        assert!(!normal.is_null());
        assert!(unsafe { print_signal(normal) }.contains("SIG"));

        let mut signals = [mixed, CsigInput(0), ptr::null_mut()];
        let outputs = unsafe { CsimplifyToNormalForm2(signals.as_mut_ptr()) };
        assert!(!outputs.is_null());
        // SAFETY: `CsimplifyToNormalForm2` returns a context-owned null-terminated array.
        let first = unsafe { *outputs };
        // SAFETY: second element exists because the input array had two elements.
        let second = unsafe { *outputs.add(1) };
        // SAFETY: third element is the terminating null pointer.
        let terminator = unsafe { *outputs.add(2) };
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert!(terminator.is_null());
        unsafe { box_ffi::freeCMemory(outputs.cast()) };
    }

    #[test]
    fn creates_source_from_signal_arrays() {
        let _guard = lock_context();
        reset_global_context();

        let gain = CString::new("gain").expect("valid label");
        let slider = CsigHSlider(
            gain.as_ptr(),
            CsigReal(0.5),
            CsigReal(0.0),
            CsigReal(1.0),
            CsigReal(0.01),
        );
        let output = CsigMul(CsigInput(0), slider);
        let mut signals = [output, ptr::null_mut()];
        let name = CString::new("SignalDSP").expect("valid name");
        let lang = CString::new("c").expect("valid lang");
        let mut error = [0_i8; 4096];
        let source = unsafe {
            CcreateSourceFromSignals(
                name.as_ptr(),
                signals.as_mut_ptr(),
                lang.as_ptr(),
                0,
                ptr::null(),
                error.as_mut_ptr(),
            )
        };
        assert!(!source.is_null());
        let source_text = unsafe { CStr::from_ptr(source) }.to_string_lossy();
        assert!(source_text.contains("SignalDSP"));
        assert!(source_text.contains("compute"));
        unsafe { box_ffi::freeCMemory(source.cast()) };

        let bad_lang = CString::new("rust").expect("valid lang");
        let failed = unsafe {
            CcreateSourceFromSignals(
                name.as_ptr(),
                signals.as_mut_ptr(),
                bad_lang.as_ptr(),
                0,
                ptr::null(),
                error.as_mut_ptr(),
            )
        };
        assert!(failed.is_null());
        let error_text = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        assert!(error_text.contains("unsupported lang"));
    }

    #[test]
    fn matches_int64_and_unsupported_doc_tables_deterministically() {
        let _guard = lock_context();
        reset_global_context();

        let value = CsigInt64(i64::from(i32::MAX) + 42);
        let mut out = 0_i64;
        assert!(CisSigInt64(value, &mut out));
        assert_eq!(out, i64::from(i32::MAX) + 42);

        assert!(!CisSigDocConstantTbl(
            value,
            ptr::null_mut(),
            ptr::null_mut()
        ));
        assert!(!CisSigDocWriteTbl(
            value,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut()
        ));
        assert!(!CisSigDocAccessTbl(value, ptr::null_mut(), ptr::null_mut()));
    }
}
