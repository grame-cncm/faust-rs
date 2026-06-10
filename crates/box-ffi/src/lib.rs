//! `box-ffi` — C/C++ export layer for Faust box construction/manipulation.
//!
//! # Overview
//! This crate exports a Faust-compatible C API (`Cbox*`, `CisBox*`, conversion
//! helpers) and companion C++ wrapper headers.
//!
//! The implementation is backed by:
//! - [`boxes::BoxBuilder`] / [`boxes::match_box`] for box construction/matching,
//! - [`tlib::TreeArena`] for hash-consed node storage,
//! - selected compiler pipeline steps for conversion APIs
//!   (`CDSPToBoxes`, `CboxesToSignals*`, `CcreateSourceFromBoxes`).
//!
//! # Context model
//! A process-global context (`createLibContext` / `destroyLibContext`) owns one
//! mutable arena and maps opaque C handles (`Box`/`Signal`) to arena node ids.
//!
//! # Safety model
//! All exported functions follow C ABI contracts. Pointers are validated at
//! entry points; invalid pointers produce null/false/0 results and, when
//! applicable, write error text in the provided error buffer.
//!
//! # Mapping status
//! The exported symbol names intentionally mirror Faust C++ headers
//! (`libfaust-box-c.h` / `libfaust-box.h`). Some advanced match predicates are
//! mapped to nearest Rust IR equivalents when an exact node kind does not
//! exist yet in this port.

#![allow(unsafe_code)]
#![allow(non_snake_case)] // FFI parity requires preserving C API symbol names.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void};

use boxes::{BoxBuilder, BoxMatch, dump_box, match_box};
use codegen::backends::c::{COptions, generate_c_module};
use codegen::backends::cpp::{CppOptions, generate_cpp_module};
use codegen::backends::interp::{InterpOptions, generate_interp_module, write_fbc};
use compiler::Compiler;
use fir::{FirId, FirStore};
use propagate::{
    ArityCache, PropagateUiOptions, box_arity_typed, make_sig_input_list, propagate_typed,
    propagate_typed_with_ui_options, try_build_flat_box,
};
use tlib::{
    NodeKind, TreeArena, TreeId, de_bruijn_to_sym, tree_to_double, tree_to_int, tree_to_str,
};
use transform::signal_fir::{RealType, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui};
use tree_ffi::{
    FfiSignalControl, FfiSignalControlKind, FfiTreeContext as BoxContext,
    read_c_string as unsafe_read_label, reset_global_context as reset_shared_context,
    with_global_context as with_ctx, write_out_handle as unsafe_write_out_box,
    write_out_int as unsafe_write_out_int, write_out_real as unsafe_write_out_real,
};
pub use tree_ffi::{SOperator, SType};
use ui::{
    ControlKind, ControlRange, ControlSpec, UiBuilder, UiProgram, UiProgramBuilder, UiRootOrigin,
    ordering_key_from_label, split_label_metadata,
};

/// FIR package exported from box/signal handles for backend FFI constructors.
///
/// This is a crate-level bridge type used by backend FFI layers (for example
/// `cranelift-ffi`) so they can reuse the exact same tree decoding and
/// signal->FIR lowering path as `box-ffi` APIs.
#[derive(Debug)]
/// Opaque FIR module handle returned by the box FFI bridge.
pub struct BoxFfiFirModule {
    /// Lowered FIR arena.
    pub store: FirStore,
    /// FIR module root id in [`Self::store`].
    pub module: FirId,
    /// Number of audio inputs inferred from the source graph.
    pub num_inputs: usize,
    /// Number of audio outputs represented by the lowered signal list.
    pub num_outputs: usize,
}

/// Infers the number of DSP inputs referenced by a propagated signal list.
fn infer_num_inputs_from_signals(arena: &TreeArena, outputs: &[TreeId]) -> usize {
    let mut max_input = None::<usize>;
    let mut stack = outputs.to_vec();
    let mut seen = std::collections::HashSet::<u32>::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node.as_u32()) {
            continue;
        }
        if let Some(tree) = arena.node(node) {
            if let NodeKind::Tag(tag_id) = &tree.kind
                && arena.tag_name(*tag_id) == Some("SIGINPUT")
                && let Some(index_id) = tree.children.get(0)
                && let Some(NodeKind::Int(idx)) = arena.kind(index_id)
                && *idx >= 0
            {
                max_input = Some(max_input.map_or(*idx as usize, |cur| cur.max(*idx as usize)));
            }
            stack.extend(tree.children.as_slice().iter().copied());
        }
    }
    max_input.map_or(0, |idx| idx.saturating_add(1))
}

/// Lowers a propagated signal root list to a standalone FIR module for FFI callers.
fn control_label_and_metadata(
    ctx: &BoxContext,
    control: &FfiSignalControl,
) -> (String, ui::UiMetadata) {
    let raw = tree_to_str(&ctx.arena, control.label).unwrap_or_default();
    split_label_metadata(raw)
}

fn control_range(ctx: &BoxContext, control: &FfiSignalControl) -> Option<ControlRange> {
    let range_for = |id: Option<TreeId>, fallback| {
        id.and_then(|id| tree_to_double(&ctx.arena, id))
            .unwrap_or(fallback)
    };
    match control.kind {
        FfiSignalControlKind::VSlider
        | FfiSignalControlKind::HSlider
        | FfiSignalControlKind::NumEntry => Some(ControlRange {
            init: range_for(control.init, 0.0),
            min: range_for(control.min, 0.0),
            max: range_for(control.max, 1.0),
            step: range_for(control.step, 0.0),
        }),
        FfiSignalControlKind::VBargraph | FfiSignalControlKind::HBargraph => Some(ControlRange {
            init: 0.0,
            min: range_for(control.min, 0.0),
            max: range_for(control.max, 1.0),
            step: 0.0,
        }),
        FfiSignalControlKind::Button
        | FfiSignalControlKind::Checkbox
        | FfiSignalControlKind::Soundfile => None,
    }
}

fn control_kind(kind: FfiSignalControlKind) -> ControlKind {
    match kind {
        FfiSignalControlKind::Button => ControlKind::Button,
        FfiSignalControlKind::Checkbox => ControlKind::Checkbox,
        FfiSignalControlKind::VSlider => ControlKind::VSlider,
        FfiSignalControlKind::HSlider => ControlKind::HSlider,
        FfiSignalControlKind::NumEntry => ControlKind::NumEntry,
        FfiSignalControlKind::VBargraph => ControlKind::VBargraph,
        FfiSignalControlKind::HBargraph => ControlKind::HBargraph,
        FfiSignalControlKind::Soundfile => ControlKind::Soundfile,
    }
}

/// Builds a synthesized UI program for Signal-only FFI entry points.
///
/// Signal constructors store UI/soundfile metadata in the shared
/// [`tree_ffi::FfiTreeContext`]. This helper turns that registry back into a
/// minimal [`UiProgram`] so normal-form preparation and source generation can
/// type/lower Signal handles without going through a Box program.
pub fn signal_only_root_ui(ctx: &BoxContext, module_name: &str) -> UiProgram {
    let mut arena = TreeArena::new();
    if ctx.signal_controls().is_empty() {
        let root = UiBuilder::new(&mut arena).vgroup(module_name, &[]);
        return UiProgram {
            arena,
            root,
            controls: Vec::new(),
            root_origin: UiRootOrigin::Synthesized,
            emit_ui: true,
        };
    }

    let mut controls = Vec::with_capacity(ctx.signal_controls().len());
    let mut builder = UiProgramBuilder::new();
    for control in ctx.signal_controls() {
        let (label, metadata) = control_label_and_metadata(ctx, control);
        let order_key = ordering_key_from_label(&label, &metadata);
        controls.push(ControlSpec {
            id: control.id,
            kind: control_kind(control.kind),
            label,
            metadata,
            range: control_range(ctx, control),
        });
        match control.kind {
            FfiSignalControlKind::Button
            | FfiSignalControlKind::Checkbox
            | FfiSignalControlKind::VSlider
            | FfiSignalControlKind::HSlider
            | FfiSignalControlKind::NumEntry => {
                builder.insert_input_control(&[], control.id, order_key);
            }
            FfiSignalControlKind::VBargraph | FfiSignalControlKind::HBargraph => {
                builder.insert_output_control(&[], control.id, order_key);
            }
            FfiSignalControlKind::Soundfile => {
                builder.insert_soundfile(&[], control.id, order_key);
            }
        }
    }

    let (mut arena, roots) = builder.finish();
    let root = UiBuilder::new(&mut arena).vgroup(module_name, &roots);
    UiProgram {
        arena,
        root,
        controls,
        root_origin: UiRootOrigin::Synthesized,
        emit_ui: true,
    }
}

fn lower_signal_roots_to_fir(
    ctx: &BoxContext,
    signal_roots: &[TreeId],
    module_name: &str,
) -> Result<BoxFfiFirModule, String> {
    if signal_roots.is_empty() {
        return Err("signal list is empty".to_owned());
    }
    let num_inputs = infer_num_inputs_from_signals(&ctx.arena, signal_roots);
    let num_outputs = signal_roots.len();
    let ui = signal_only_root_ui(ctx, module_name);
    let lowered = compile_signals_to_fir_fastlane_with_ui(
        &ctx.arena,
        signal_roots,
        num_inputs,
        num_outputs,
        &ui,
        &SignalFirOptions {
            module_name: module_name.to_owned(),
            strict_mode: true,
            real_type: RealType::Float32,
            ..SignalFirOptions::default()
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(BoxFfiFirModule {
        store: lowered.store,
        module: lowered.module,
        num_inputs,
        num_outputs,
    })
}

/// Decodes a raw C array of signal handles into Rust `TreeId`s.
fn decode_signal_handle_array(
    ctx: &BoxContext,
    signals: *mut c_void,
) -> Result<Vec<TreeId>, String> {
    if signals.is_null() {
        return Err("null signals pointer".to_owned());
    }
    let mut out = Vec::new();
    let mut cur = signals.cast::<*mut c_void>();
    loop {
        // SAFETY: caller provides a valid null-terminated signal handle array.
        let handle = unsafe { *cur };
        if handle.is_null() {
            break;
        }
        let Some(id) = ctx.decode(handle) else {
            return Err("unknown signal handle in array".to_owned());
        };
        out.push(id);
        // SAFETY: same as array dereference contract above.
        cur = unsafe { cur.add(1) };
    }
    if out.is_empty() {
        return Err("signal list is empty".to_owned());
    }
    Ok(out)
}

/// Lower one `Box` handle to FIR for backend-side factory construction.
///
/// This function is intended for Rust backend FFI crates and mirrors the exact
/// lowering used by `CcreateSourceFromBoxes`.
///
/// # Safety
/// `box_ptr` must be a handle created by this `box-ffi` context API.
pub unsafe fn export_fir_from_box_handle(
    module_name: &str,
    box_ptr: *mut c_void,
) -> Result<BoxFfiFirModule, String> {
    with_ctx(|ctx| {
        let Some(box_id) = ctx.decode(box_ptr) else {
            return Err("null or unknown box pointer".to_owned());
        };
        let flat = try_build_flat_box(&ctx.arena, box_id).map_err(|e| e.to_string())?;
        let mut cache = ArityCache::new();
        let arity = box_arity_typed(&ctx.arena, flat, &mut cache).map_err(|e| e.to_string())?;
        let inputs = make_sig_input_list(&mut ctx.arena, arity.inputs);
        let propagated = propagate_typed_with_ui_options(
            &mut ctx.arena,
            flat,
            &inputs,
            &mut cache,
            &PropagateUiOptions::new(module_name),
        )
        .map_err(|e| e.to_string())?;
        let lowered = compile_signals_to_fir_fastlane_with_ui(
            &ctx.arena,
            &propagated.signals,
            arity.inputs,
            arity.outputs,
            &propagated.ui,
            &SignalFirOptions {
                module_name: module_name.to_owned(),
                strict_mode: true,
                real_type: RealType::Float32,
                ..SignalFirOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(BoxFfiFirModule {
            store: lowered.store,
            module: lowered.module,
            num_inputs: arity.inputs,
            num_outputs: arity.outputs,
        })
    })
}

/// Lower one null-terminated signal handle array to FIR for backend-side
/// factory construction.
///
/// # Safety
/// - `signals` must point to a valid null-terminated `*mut c_void` array.
/// - Each non-null entry must be a signal handle created by this `box-ffi`
///   context API.
pub unsafe fn export_fir_from_signal_array_handle(
    module_name: &str,
    signals: *mut c_void,
) -> Result<BoxFfiFirModule, String> {
    with_ctx(|ctx| {
        let roots = decode_signal_handle_array(ctx, signals)?;
        lower_signal_roots_to_fir(ctx, &roots, module_name)
    })
}

/// Render a lowered FFI FIR module into one supported textual backend.
pub fn render_fir_module_source(
    fir: &BoxFfiFirModule,
    lang: &str,
    module_name: &str,
) -> Result<String, String> {
    match lang {
        "c" => generate_c_module(
            &fir.store,
            fir.module,
            &COptions {
                class_name: Some(module_name.to_owned()),
                ..COptions::default()
            },
        )
        .map_err(|e| e.to_string()),
        "cpp" => generate_cpp_module(
            &fir.store,
            fir.module,
            &CppOptions {
                class_name: Some(module_name.to_owned()),
                ..CppOptions::default()
            },
        )
        .map_err(|e| e.to_string()),
        "fir" => Ok(fir::dump_fir(&fir.store, fir.module)),
        "interp" => {
            let factory = generate_interp_module::<f32>(
                &fir.store,
                fir.module,
                &InterpOptions {
                    module_name: Some(module_name.to_owned()),
                    ..InterpOptions::default()
                },
            )
            .map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            write_fbc(&factory, &mut buf, false).map_err(|e| e.to_string())?;
            String::from_utf8(buf).map_err(|e| e.to_string())
        }
        _ => Err(format!(
            "unsupported lang '{lang}' (expected c, cpp, fir, or interp)"
        )),
    }
}

/// Compile one null-terminated Signal handle array to target source code.
///
/// This is the Rust-side bridge used by `signal-ffi`'s
/// `CcreateSourceFromSignals` and mirrors `CcreateSourceFromBoxes` after Box
/// propagation.
///
/// # Safety
/// `signals` must point to a valid null-terminated `*mut c_void` array whose
/// entries are handles from the shared FFI context.
pub unsafe fn export_source_from_signal_array_handle(
    name_app: &str,
    signals: *mut c_void,
    lang: &str,
    argv: &[String],
) -> Result<String, String> {
    let parsed = utils::parse_ffi_compile_args(argv)?;
    let module_name = parsed
        .module_name
        .clone()
        .unwrap_or_else(|| name_app.to_owned());
    // SAFETY: caller upholds the signal-array contract for this helper.
    let fir = unsafe { export_fir_from_signal_array_handle(&module_name, signals)? };
    render_fir_module_source(&fir, lang, &module_name)
}

/// Builds `x : op` helper shape used by unary `...Aux` APIs.
fn unary_aux(ctx: &mut BoxContext, x: TreeId, op: TreeId) -> TreeId {
    let mut b = BoxBuilder::new(&mut ctx.arena);
    b.seq(x, op)
}

/// Builds `(left, right) : op` helper shape used by binary `...Aux` APIs.
fn binary_aux(ctx: &mut BoxContext, left: TreeId, right: TreeId, op: TreeId) -> TreeId {
    let mut b = BoxBuilder::new(&mut ctx.arena);
    let par = b.par(left, right);
    b.seq(par, op)
}

/// Builds `(a, b0, c) : op` helper shape used by ternary `...Aux` APIs.
fn ternary_aux(ctx: &mut BoxContext, a: TreeId, b0: TreeId, c: TreeId, op: TreeId) -> TreeId {
    let mut b = BoxBuilder::new(&mut ctx.arena);
    let par = b.par(a, b0);
    let par = b.par(par, c);
    b.seq(par, op)
}

/// Builds `(a, b0, c, d) : op` helper shape used by quaternary `...Aux` APIs.
fn quaternary_aux(
    ctx: &mut BoxContext,
    a: TreeId,
    b0: TreeId,
    c: TreeId,
    d: TreeId,
    op: TreeId,
) -> TreeId {
    let mut b = BoxBuilder::new(&mut ctx.arena);
    let par = b.par(a, b0);
    let par = b.par(par, c);
    let par = b.par(par, d);
    b.seq(par, op)
}

/// Interns one optional C label in the shared FFI context.
fn label_tree(ctx: &mut BoxContext, label: *const c_char) -> Option<TreeId> {
    unsafe { ctx.label_tree(label) }
}

/// Reads one optional C string and converts it to owned UTF-8.
fn read_label(label: *const c_char) -> Option<String> {
    unsafe { unsafe_read_label(label) }
}

/// Writes one box handle result to an optional out-pointer.
fn write_out_box(ctx: &mut BoxContext, out: *mut *mut c_void, value: TreeId) {
    unsafe { unsafe_write_out_box(ctx, out, value) }
}

/// Writes one integer result to an optional out-pointer.
fn write_out_int(out: *mut c_int, value: i32) {
    unsafe { unsafe_write_out_int(out, value) }
}

/// Writes one floating-point result to an optional out-pointer.
fn write_out_real(out: *mut f64, value: f64) {
    unsafe { unsafe_write_out_real(out, value) }
}

/// Maps [`SOperator`] values to primitive box constructor nodes.
fn op_primitive(builder: &mut BoxBuilder<'_>, op: SOperator) -> TreeId {
    match op {
        SOperator::kAdd => builder.add(),
        SOperator::kSub => builder.sub(),
        SOperator::kMul => builder.mul(),
        SOperator::kDiv => builder.div(),
        SOperator::kRem => builder.rem(),
        SOperator::kLsh => builder.lsh(),
        SOperator::kARsh => builder.rsh(),
        SOperator::kLRsh => builder.lrsh(),
        SOperator::kGT => builder.gt(),
        SOperator::kLT => builder.lt(),
        SOperator::kGE => builder.ge(),
        SOperator::kLE => builder.le(),
        SOperator::kEQ => builder.eq(),
        SOperator::kNE => builder.ne(),
        SOperator::kAND => builder.and(),
        SOperator::kOR => builder.or(),
        SOperator::kXOR => builder.xor(),
    }
}

/// Deep-imports one source tree from another arena into the local context arena.
///
/// This preserves structural sharing through `memo` while rebuilding nodes in
/// the destination arena.
fn import_tree_rec(
    src: &TreeArena,
    dst: &mut TreeArena,
    node: TreeId,
    memo: &mut HashMap<u32, TreeId>,
) -> Option<TreeId> {
    if let Some(mapped) = memo.get(&node.as_u32()).copied() {
        return Some(mapped);
    }
    let src_node = src.node(node)?;
    let mut mapped_children = Vec::with_capacity(src_node.children.len());
    for child in src_node.children.as_slice() {
        mapped_children.push(import_tree_rec(src, dst, *child, memo)?);
    }
    let mapped = dst.intern(src_node.kind.clone(), &mapped_children);
    memo.insert(node.as_u32(), mapped);
    Some(mapped)
}

#[unsafe(no_mangle)]
/// Initializes/reinitializes the process-global box compilation context.
///
/// # Safety
/// This function does not dereference caller pointers.
pub extern "C" fn createLibContext() {
    reset_shared_context();
}

#[unsafe(no_mangle)]
/// Destroys the global context by resetting all arena/handle state.
///
/// # Safety
/// This function does not dereference caller pointers.
pub extern "C" fn destroyLibContext() {
    reset_shared_context();
}

#[cfg_attr(feature = "standalone-capi-globals", unsafe(no_mangle))]
/// Free a heap-allocated C string returned by this library.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by this library for a
/// string result.
pub unsafe extern "C" fn freeCMemory(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let freed_array = with_ctx(|ctx| ctx.free_if_handle_ptr_array(ptr));
    if !freed_array {
        unsafe { utils::free_c_memory_c_string_only(ptr) }
    }
}

#[unsafe(no_mangle)]
/// Predicate equivalent to Faust `isNil`.
///
/// # Safety
/// `b` may be null or a valid box handle.
pub extern "C" fn CisNil(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return true;
        };
        ctx.arena.is_nil(id)
    })
}

#[unsafe(no_mangle)]
/// Converts a tree atom to integer (`0` when non-integer or out-of-range).
///
/// # Safety
/// `b` may be null or a valid tree handle.
pub extern "C" fn Ctree2int(b: *mut c_void) -> c_int {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return 0;
        };
        tree_to_int(&ctx.arena, id)
            .and_then(|v| c_int::try_from(v).ok())
            .unwrap_or_default()
    })
}

#[unsafe(no_mangle)]
/// Converts a tree atom to text (caller frees returned C string with `freeCMemory`).
///
/// # Safety
/// `b` may be null or a valid tree handle.
pub extern "C" fn Ctree2str(b: *mut c_void) -> *const c_char {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return std::ptr::null();
        };
        let Some(name) = tree_to_str(&ctx.arena, id) else {
            eprintln!(
                "ERROR : the parameter must be a symbol known at compile time : {:?}",
                ctx.arena.kind(id)
            );
            return std::ptr::null();
        };
        utils::alloc_c_string(name) as *const c_char
    })
}

#[unsafe(no_mangle)]
/// Returns user data attached to a box node (currently unsupported, always null).
///
/// # Safety
/// `b` may be null or a valid tree handle.
pub extern "C" fn CgetUserData(_b: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
/// Dumps one box subtree to text (caller frees result with `freeCMemory`).
///
/// # Safety
/// `box_ptr` may be null or a valid tree handle.
pub extern "C" fn CprintBox(box_ptr: *mut c_void, _shared: bool, _max_size: c_int) -> *mut c_char {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(box_ptr) else {
            return std::ptr::null_mut();
        };
        let dumped = dump_box(&ctx.arena, id);
        utils::alloc_c_string(&dumped)
    })
}

#[unsafe(no_mangle)]
/// Dumps one signal subtree to text.
///
/// In this implementation signals and boxes share the same underlying tree model.
///
/// # Safety
/// `sig_ptr` may be null or a valid tree handle.
pub extern "C" fn CprintSignal(sig_ptr: *mut c_void, shared: bool, max_size: c_int) -> *mut c_char {
    CprintBox(sig_ptr, shared, max_size)
}

#[unsafe(no_mangle)]
/// Creates one integer constant box.
///
/// # Safety
/// This function does not dereference caller pointers.
pub extern "C" fn CboxInt(n: c_int) -> *mut c_void {
    with_ctx(|ctx| {
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.int(n)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates one floating-point constant box.
///
/// # Safety
/// This function does not dereference caller pointers.
pub extern "C" fn CboxReal(n: f64) -> *mut c_void {
    with_ctx(|ctx| {
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.real(n)
        };
        ctx.encode(id)
    })
}

macro_rules! prim0 {
    ($name:ident, $method:ident) => {
        #[unsafe(no_mangle)]
        #[doc = concat!(
                                                            "Constructs primitive box node `",
                                                            stringify!($name),
                                                            "` (no operand form)."
                                                        )]
        ///
        /// # Safety
        /// This function does not dereference raw pointers.
        pub extern "C" fn $name() -> *mut c_void {
            with_ctx(|ctx| {
                let id = {
                    let mut b = BoxBuilder::new(&mut ctx.arena);
                    b.$method()
                };
                ctx.encode(id)
            })
        }
    };
}

macro_rules! prim2 {
    ($name:ident, $method:ident) => {
        #[unsafe(no_mangle)]
        #[doc = concat!(
                                                                    "Constructs composite node `",
                                                                    stringify!($name),
                                                                    "` from two input box handles."
                                                                )]
        ///
        /// # Safety
        /// `x` and `y` must be valid box handles created by this library.
        pub extern "C" fn $name(x: *mut c_void, y: *mut c_void) -> *mut c_void {
            with_ctx(|ctx| {
                let (Some(x), Some(y)) = (ctx.decode(x), ctx.decode(y)) else {
                    return std::ptr::null_mut();
                };
                let id = {
                    let mut b = BoxBuilder::new(&mut ctx.arena);
                    b.$method(x, y)
                };
                ctx.encode(id)
            })
        }
    };
}

macro_rules! unop {
    ($prim:ident, $aux:ident, $method:ident) => {
        prim0!($prim, $method);
        #[unsafe(no_mangle)]
        #[doc = concat!(
                                                                    "Builds auxiliary unary form `",
                                                                    stringify!($aux),
                                                                    "` as `(x) : ",
                                                                    stringify!($prim),
                                                                    "`."
                                                                )]
        ///
        /// # Safety
        /// `x` must be a valid box handle created by this library.
        pub extern "C" fn $aux(x: *mut c_void) -> *mut c_void {
            with_ctx(|ctx| {
                let Some(x) = ctx.decode(x) else {
                    return std::ptr::null_mut();
                };
                let op = {
                    let mut b = BoxBuilder::new(&mut ctx.arena);
                    b.$method()
                };
                let id = unary_aux(ctx, x, op);
                ctx.encode(id)
            })
        }
    };
}

macro_rules! binop {
    ($prim:ident, $aux:ident, $method:ident) => {
        prim0!($prim, $method);
        #[unsafe(no_mangle)]
        #[doc = concat!(
                                                            "Builds auxiliary binary form `",
                                                            stringify!($aux),
                                                            "` as `(x, y) : ",
                                                            stringify!($prim),
                                                            "`."
                                                        )]
        ///
        /// # Safety
        /// `x` and `y` must be valid box handles created by this library.
        pub extern "C" fn $aux(x: *mut c_void, y: *mut c_void) -> *mut c_void {
            with_ctx(|ctx| {
                let (Some(x), Some(y)) = (ctx.decode(x), ctx.decode(y)) else {
                    return std::ptr::null_mut();
                };
                let op = {
                    let mut b = BoxBuilder::new(&mut ctx.arena);
                    b.$method()
                };
                let id = binary_aux(ctx, x, y, op);
                ctx.encode(id)
            })
        }
    };
}

prim0!(CboxWire, wire);
prim0!(CboxCut, cut);
prim2!(CboxSeq, seq);
prim2!(CboxPar, par);
prim2!(CboxSplit, split);
prim2!(CboxMerge, merge);
prim2!(CboxRec, rec);
prim2!(CboxFad, forward_ad);
prim2!(CboxRad, reverse_ad);

#[unsafe(no_mangle)]
/// Creates a 3-way parallel composition.
///
/// # Safety
/// `x`, `y`, and `z` must be valid box handles created by this library.
pub extern "C" fn CboxPar3(x: *mut c_void, y: *mut c_void, z: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(x), Some(y), Some(z)) = (ctx.decode(x), ctx.decode(y), ctx.decode(z)) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            let p = b.par(x, y);
            b.par(p, z)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a 4-way parallel composition.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxPar4(
    a: *mut c_void,
    b0: *mut c_void,
    c: *mut c_void,
    d: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(a), Some(b0), Some(c), Some(d)) =
            (ctx.decode(a), ctx.decode(b0), ctx.decode(c), ctx.decode(d))
        else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            let p = b.par(a, b0);
            let p = b.par(p, c);
            b.par(p, d)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a 5-way parallel composition.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxPar5(
    a: *mut c_void,
    b0: *mut c_void,
    c: *mut c_void,
    d: *mut c_void,
    e: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(a), Some(b0), Some(c), Some(d), Some(e)) = (
            ctx.decode(a),
            ctx.decode(b0),
            ctx.decode(c),
            ctx.decode(d),
            ctx.decode(e),
        ) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            let p = b.par(a, b0);
            let p = b.par(p, c);
            let p = b.par(p, d);
            b.par(p, e)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a route box from `n`, `m`, and route specification.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxRoute(n: *mut c_void, m: *mut c_void, r: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(n), Some(m), Some(r)) = (ctx.decode(n), ctx.decode(m), ctx.decode(r)) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.route(n, m, r)
        };
        ctx.encode(id)
    })
}

prim0!(CboxDelay, delay);
unop!(CboxIntCast, CboxIntCastAux, int_cast);
unop!(CboxFloatCast, CboxFloatCastAux, float_cast);
unop!(CboxAbs, CboxAbsAux, abs);
unop!(CboxAcos, CboxAcosAux, acos);
unop!(CboxTan, CboxTanAux, tan);
unop!(CboxSqrt, CboxSqrtAux, sqrt);
unop!(CboxSin, CboxSinAux, sin);
unop!(CboxRint, CboxRintAux, rint);
unop!(CboxRound, CboxRoundAux, round);
unop!(CboxLog, CboxLogAux, log);
unop!(CboxLog10, CboxLog10Aux, log10);
unop!(CboxFloor, CboxFloorAux, floor);
unop!(CboxExp, CboxExpAux, exp);
unop!(CboxExp10, CboxExp10Aux, exp10);
unop!(CboxCos, CboxCosAux, cos);
unop!(CboxCeil, CboxCeilAux, ceil);
unop!(CboxAtan, CboxAtanAux, atan);
unop!(CboxAsin, CboxAsinAux, asin);

binop!(CboxAdd, CboxAddAux, add);
binop!(CboxSub, CboxSubAux, sub);
binop!(CboxMul, CboxMulAux, mul);
binop!(CboxDiv, CboxDivAux, div);
binop!(CboxRem, CboxRemAux, rem);
binop!(CboxLeftShift, CboxLeftShiftAux, lsh);
binop!(CboxLRightShift, CboxLRightShiftAux, lrsh);
binop!(CboxARightShift, CboxARightShiftAux, rsh);
binop!(CboxGT, CboxGTAux, gt);
binop!(CboxLT, CboxLTAux, lt);
binop!(CboxGE, CboxGEAux, ge);
binop!(CboxLE, CboxLEAux, le);
binop!(CboxEQ, CboxEQAux, eq);
binop!(CboxNE, CboxNEAux, ne);
binop!(CboxAND, CboxANDAux, and);
binop!(CboxOR, CboxORAux, or);
binop!(CboxXOR, CboxXORAux, xor);
binop!(CboxRemainder, CboxRemainderAux, remainder);
binop!(CboxPow, CboxPowAux, pow);
binop!(CboxMin, CboxMinAux, min);
binop!(CboxMax, CboxMaxAux, max);
binop!(CboxFmod, CboxFmodAux, fmod);
binop!(CboxAtan2, CboxAtan2Aux, atan2);

prim0!(CboxReadOnlyTable, read_only_table);
prim0!(CboxWriteReadTable, write_read_table);
prim0!(CboxSelect2, select2);
prim0!(CboxSelect3, select3);
prim0!(CboxAttach, attach);

#[unsafe(no_mangle)]
/// Builds read-only table auxiliary form `(n, init, ridx) : readOnlyTable`.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxReadOnlyTableAux(
    n: *mut c_void,
    init: *mut c_void,
    ridx: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(n), Some(init), Some(ridx)) = (ctx.decode(n), ctx.decode(init), ctx.decode(ridx))
        else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.read_only_table()
        };
        let id = ternary_aux(ctx, n, init, ridx, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds write-read table auxiliary form
/// `(n, init, widx, wsig, ridx) : writeReadTable`.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxWriteReadTableAux(
    n: *mut c_void,
    init: *mut c_void,
    widx: *mut c_void,
    wsig: *mut c_void,
    ridx: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(n), Some(init), Some(widx), Some(wsig), Some(ridx)) = (
            ctx.decode(n),
            ctx.decode(init),
            ctx.decode(widx),
            ctx.decode(wsig),
            ctx.decode(ridx),
        ) else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.write_read_table()
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            let par = b.par(n, init);
            let par = b.par(par, widx);
            let par = b.par(par, wsig);
            let par = b.par(par, ridx);
            b.seq(par, op)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds select2 auxiliary form `(selector, b1, b2) : select2`.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxSelect2Aux(
    selector: *mut c_void,
    b1: *mut c_void,
    b2: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(selector), Some(b1), Some(b2)) =
            (ctx.decode(selector), ctx.decode(b1), ctx.decode(b2))
        else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.select2()
        };
        let id = ternary_aux(ctx, selector, b1, b2, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds select3 auxiliary form `(selector, b1, b2, b3) : select3`.
///
/// # Safety
/// All arguments must be valid box handles created by this library.
pub extern "C" fn CboxSelect3Aux(
    selector: *mut c_void,
    b1: *mut c_void,
    b2: *mut c_void,
    b3: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(selector), Some(b1), Some(b2), Some(b3)) = (
            ctx.decode(selector),
            ctx.decode(b1),
            ctx.decode(b2),
            ctx.decode(b3),
        ) else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.select3()
        };
        let id = quaternary_aux(ctx, selector, b1, b2, b3, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds attach auxiliary form `(b1, b2) : attach`.
///
/// # Safety
/// `b1` and `b2` must be valid box handles created by this library.
pub extern "C" fn CboxAttachAux(b1: *mut c_void, b2: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(b1), Some(b2)) = (ctx.decode(b1), ctx.decode(b2)) else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.attach()
        };
        let id = binary_aux(ctx, b1, b2, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Build a waveform box from a null-terminated array of box handles.
///
/// # Safety
/// `wf` must point to a null-terminated array of valid box handles.
pub unsafe extern "C" fn CboxWaveform(wf: *const *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        if wf.is_null() {
            return std::ptr::null_mut();
        }
        let mut values = Vec::new();
        let mut idx = 0usize;
        loop {
            let ptr = unsafe { *wf.add(idx) };
            if ptr.is_null() {
                break;
            }
            let Some(id) = ctx.decode(ptr) else {
                return std::ptr::null_mut();
            };
            values.push(id);
            idx += 1;
        }
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.waveform(&values)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a soundfile box from label + channel-count expression.
///
/// # Safety
/// `label` must be a valid C string and `chan` a valid box handle.
pub extern "C" fn CboxSoundfile(label: *const c_char, chan: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(chan) = ctx.decode(chan) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.soundfile(lbl, chan)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a button UI box.
///
/// # Safety
/// `label` must be a valid C string.
pub extern "C" fn CboxButton(label: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.button(lbl)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a checkbox UI box.
///
/// # Safety
/// `label` must be a valid C string.
pub extern "C" fn CboxCheckbox(label: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.checkbox(lbl)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a vertical slider UI box.
///
/// # Safety
/// `label` must be a valid C string; numeric arguments must be valid box handles.
pub extern "C" fn CboxVSlider(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(init), Some(min), Some(max), Some(step)) = (
            ctx.decode(init),
            ctx.decode(min),
            ctx.decode(max),
            ctx.decode(step),
        ) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.vslider(lbl, init, min, max, step)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a horizontal slider UI box.
///
/// # Safety
/// `label` must be a valid C string; numeric arguments must be valid box handles.
pub extern "C" fn CboxHSlider(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(init), Some(min), Some(max), Some(step)) = (
            ctx.decode(init),
            ctx.decode(min),
            ctx.decode(max),
            ctx.decode(step),
        ) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.hslider(lbl, init, min, max, step)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a numeric entry UI box.
///
/// # Safety
/// `label` must be a valid C string; numeric arguments must be valid box handles.
pub extern "C" fn CboxNumEntry(
    label: *const c_char,
    init: *mut c_void,
    min: *mut c_void,
    max: *mut c_void,
    step: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(init), Some(min), Some(max), Some(step)) = (
            ctx.decode(init),
            ctx.decode(min),
            ctx.decode(max),
            ctx.decode(step),
        ) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.num_entry(lbl, init, min, max, step)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a vertical bargraph UI box.
///
/// # Safety
/// `label` must be a valid C string; range arguments must be valid box handles.
pub extern "C" fn CboxVBargraph(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(min), Some(max)) = (ctx.decode(min), ctx.decode(max)) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.vbargraph(lbl, min, max)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds vertical bargraph auxiliary form `(x) : vbargraph`.
///
/// # Safety
/// `label` must be a valid C string; box arguments must be valid handles.
pub extern "C" fn CboxVBargraphAux(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
    x: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(min), Some(max), Some(x)) = (ctx.decode(min), ctx.decode(max), ctx.decode(x))
        else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let bar = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.vbargraph(lbl, min, max)
        };
        let id = unary_aux(ctx, x, bar);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a horizontal bargraph UI box.
///
/// # Safety
/// `label` must be a valid C string; range arguments must be valid box handles.
pub extern "C" fn CboxHBargraph(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(min), Some(max)) = (ctx.decode(min), ctx.decode(max)) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.hbargraph(lbl, min, max)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds horizontal bargraph auxiliary form `(x) : hbargraph`.
///
/// # Safety
/// `label` must be a valid C string; box arguments must be valid handles.
pub extern "C" fn CboxHBargraphAux(
    label: *const c_char,
    min: *mut c_void,
    max: *mut c_void,
    x: *mut c_void,
) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(min), Some(max), Some(x)) = (ctx.decode(min), ctx.decode(max), ctx.decode(x))
        else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let bar = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.hbargraph(lbl, min, max)
        };
        let id = unary_aux(ctx, x, bar);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a vertical group UI box.
///
/// # Safety
/// `label` must be a valid C string and `group` a valid box handle.
pub extern "C" fn CboxVGroup(label: *const c_char, group: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(group) = ctx.decode(group) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.vgroup(lbl, group)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a horizontal group UI box.
///
/// # Safety
/// `label` must be a valid C string and `group` a valid box handle.
pub extern "C" fn CboxHGroup(label: *const c_char, group: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(group) = ctx.decode(group) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.hgroup(lbl, group)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a tab group UI box.
///
/// # Safety
/// `label` must be a valid C string and `group` a valid box handle.
pub extern "C" fn CboxTGroup(label: *const c_char, group: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(group) = ctx.decode(group) else {
            return std::ptr::null_mut();
        };
        let Some(lbl) = label_tree(ctx, label) else {
            return std::ptr::null_mut();
        };
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.tgroup(lbl, group)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates one primitive binary-op box node.
///
/// # Safety
/// `op` must be one of the valid [`SOperator`] enum values.
pub extern "C" fn CboxBinOp(op: SOperator) -> *mut c_void {
    with_ctx(|ctx| {
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            op_primitive(&mut b, op)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds binary-op auxiliary form `(b1, b2) : op`.
///
/// # Safety
/// `op` must be valid and both box arguments must be valid handles.
pub extern "C" fn CboxBinOpAux(op: SOperator, b1: *mut c_void, b2: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(b1), Some(b2)) = (ctx.decode(b1), ctx.decode(b2)) else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            op_primitive(&mut b, op)
        };
        let id = binary_aux(ctx, b1, b2, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Build a foreign-function box wrapper.
///
/// # Safety
/// `names` must be non-null and point to a valid array where the first entry
/// is a valid C string. `incfile` and `libfile` must be valid C strings.
pub unsafe extern "C" fn CboxFFun(
    rtype: SType,
    names: *const *const c_char,
    _atypes: *const SType,
    incfile: *const c_char,
    libfile: *const c_char,
) -> *mut c_void {
    with_ctx(|ctx| {
        if names.is_null() {
            return std::ptr::null_mut();
        }
        let primary_name = unsafe { *names };
        let Some(name) = read_label(primary_name) else {
            return std::ptr::null_mut();
        };
        let Some(inc) = read_label(incfile) else {
            return std::ptr::null_mut();
        };
        let Some(lib) = read_label(libfile) else {
            return std::ptr::null_mut();
        };

        let ty = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            match rtype {
                SType::kSInt => b.int(0),
                SType::kSReal => b.int(1),
            }
        };

        let signature = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            let name_box = b.ident(&name);
            let nil = ctx.arena.nil();
            let names = ctx.arena.cons(name_box, nil);
            let payload = ctx.arena.cons(names, nil);
            ctx.arena.cons(ty, payload)
        };
        let inc_box = ctx.arena.symbol(inc);
        let lib_box = ctx.arena.symbol(lib);
        let ff = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.ffunction(signature, inc_box, lib_box)
        };
        let wrapped = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.ffun(ff)
        };
        ctx.encode(wrapped)
    })
}

#[unsafe(no_mangle)]
/// Creates a foreign constant box.
///
/// # Safety
/// `name` and `incfile` must be valid C strings.
pub extern "C" fn CboxFConst(
    ty: SType,
    name: *const c_char,
    incfile: *const c_char,
) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(name) = read_label(name) else {
            return std::ptr::null_mut();
        };
        let Some(inc) = read_label(incfile) else {
            return std::ptr::null_mut();
        };
        let ty = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            match ty {
                SType::kSInt => b.int(0),
                SType::kSReal => b.int(1),
            }
        };
        let name_box = ctx.arena.symbol(name);
        let file_box = ctx.arena.symbol(inc);
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.fconst(ty, name_box, file_box)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Creates a foreign variable box.
///
/// # Safety
/// `name` and `incfile` must be valid C strings.
pub extern "C" fn CboxFVar(ty: SType, name: *const c_char, incfile: *const c_char) -> *mut c_void {
    with_ctx(|ctx| {
        let Some(name) = read_label(name) else {
            return std::ptr::null_mut();
        };
        let Some(inc) = read_label(incfile) else {
            return std::ptr::null_mut();
        };
        let ty = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            match ty {
                SType::kSInt => b.int(0),
                SType::kSReal => b.int(1),
            }
        };
        let name_box = ctx.arena.symbol(name);
        let file_box = ctx.arena.symbol(inc);
        let id = {
            let mut b = BoxBuilder::new(&mut ctx.arena);
            b.fvar(ty, name_box, file_box)
        };
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Builds delay auxiliary form `(b, del) : delay`.
///
/// # Safety
/// Both arguments must be valid box handles.
pub extern "C" fn CboxDelayAux(b: *mut c_void, del: *mut c_void) -> *mut c_void {
    with_ctx(|ctx| {
        let (Some(b), Some(del)) = (ctx.decode(b), ctx.decode(del)) else {
            return std::ptr::null_mut();
        };
        let op = {
            let mut builder = BoxBuilder::new(&mut ctx.arena);
            builder.delay()
        };
        let id = binary_aux(ctx, b, del, op);
        ctx.encode(id)
    })
}

#[unsafe(no_mangle)]
/// Matches `abstr(slot, body)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxAbstr(t: *mut c_void, x: *mut *mut c_void, y: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Abstr(a, b) = match_box(&ctx.arena, id) {
            write_out_box(ctx, x, a);
            write_out_box(ctx, y, b);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `access(expr, ident)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxAccess(
    t: *mut c_void,
    exp: *mut *mut c_void,
    id_out: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Access(exp_id, ident) = match_box(&ctx.arena, id) {
            write_out_box(ctx, exp, exp_id);
            write_out_box(ctx, id_out, ident);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `appl(fun, args)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxAppl(t: *mut c_void, x: *mut *mut c_void, y: *mut *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Appl(a, b) = match_box(&ctx.arena, id) {
            write_out_box(ctx, x, a);
            write_out_box(ctx, y, b);
            true
        } else {
            false
        }
    })
}

macro_rules! match_unary_out {
    ($fn_name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        #[doc = concat!("Matches box variant `", stringify!($fn_name), "` and returns one child.")]
        ///
        /// # Safety
        /// All pointers must be null or valid writable pointers/handles.
        pub extern "C" fn $fn_name(b: *mut c_void, out: *mut *mut c_void) -> bool {
            with_ctx(|ctx| {
                let Some(id) = ctx.decode(b) else {
                    return false;
                };
                if let $variant(v) = match_box(&ctx.arena, id) {
                    write_out_box(ctx, out, v);
                    true
                } else {
                    false
                }
            })
        }
    };
}

macro_rules! match_binary_out {
    ($fn_name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        #[doc = concat!("Matches box variant `", stringify!($fn_name), "` and returns two children.")]
        ///
        /// # Safety
        /// All pointers must be null or valid writable pointers/handles.
        pub extern "C" fn $fn_name(
            b: *mut c_void,
            o1: *mut *mut c_void,
            o2: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(id) = ctx.decode(b) else {
                    return false;
                };
                if let $variant(v1, v2) = match_box(&ctx.arena, id) {
                    write_out_box(ctx, o1, v1);
                    write_out_box(ctx, o2, v2);
                    true
                } else {
                    false
                }
            })
        }
    };
}

macro_rules! match_ternary_out {
    ($fn_name:ident, $variant:path) => {
        #[unsafe(no_mangle)]
        #[doc = concat!(
                                                                    "Matches box variant `",
                                                                    stringify!($fn_name),
                                                                    "` and returns three children."
                                                                )]
        ///
        /// # Safety
        /// All pointers must be null or valid writable pointers/handles.
        pub extern "C" fn $fn_name(
            b: *mut c_void,
            o1: *mut *mut c_void,
            o2: *mut *mut c_void,
            o3: *mut *mut c_void,
        ) -> bool {
            with_ctx(|ctx| {
                let Some(id) = ctx.decode(b) else {
                    return false;
                };
                if let $variant(v1, v2, v3) = match_box(&ctx.arena, id) {
                    write_out_box(ctx, o1, v1);
                    write_out_box(ctx, o2, v2);
                    write_out_box(ctx, o3, v3);
                    true
                } else {
                    false
                }
            })
        }
    };
}

match_unary_out!(CisBoxButton, BoxMatch::Button);
match_unary_out!(CisBoxCase, BoxMatch::Case);
match_unary_out!(CisBoxCheckbox, BoxMatch::Checkbox);
match_unary_out!(CisBoxComponent, BoxMatch::Component);
match_unary_out!(CisBoxFFun, BoxMatch::FFun);
match_unary_out!(CisBoxInputs, BoxMatch::Inputs);
match_unary_out!(CisBoxLibrary, BoxMatch::Library);
match_unary_out!(CisBoxOutputs, BoxMatch::Outputs);
match_unary_out!(CisBoxPatternVar, BoxMatch::PatternVar);

match_binary_out!(CisBoxHGroup, BoxMatch::HGroup);
match_binary_out!(CisBoxMerge, BoxMatch::Merge);
match_binary_out!(CisBoxPar, BoxMatch::Par);
match_binary_out!(CisBoxRec, BoxMatch::Rec);
match_binary_out!(CisBoxSeq, BoxMatch::Seq);
match_binary_out!(CisBoxSplit, BoxMatch::Split);
match_binary_out!(CisBoxTGroup, BoxMatch::TGroup);
match_binary_out!(CisBoxVGroup, BoxMatch::VGroup);
match_binary_out!(CisBoxWithLocalDef, BoxMatch::WithLocalDef);

match_ternary_out!(CisBoxIPar, BoxMatch::IPar);
match_ternary_out!(CisBoxIProd, BoxMatch::IProd);
match_ternary_out!(CisBoxISeq, BoxMatch::ISeq);
match_ternary_out!(CisBoxISum, BoxMatch::ISum);
match_ternary_out!(CisBoxRoute, BoxMatch::Route);

#[unsafe(no_mangle)]
/// Matches `fconst(type, name, file)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxFConst(
    b: *mut c_void,
    ty: *mut *mut c_void,
    name: *mut *mut c_void,
    file: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::FConst(t, n, f) = match_box(&ctx.arena, id) {
            write_out_box(ctx, ty, t);
            write_out_box(ctx, name, n);
            write_out_box(ctx, file, f);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `fvar(type, name, file)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxFVar(
    b: *mut c_void,
    ty: *mut *mut c_void,
    name: *mut *mut c_void,
    file: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::FVar(t, n, f) = match_box(&ctx.arena, id) {
            write_out_box(ctx, ty, t);
            write_out_box(ctx, name, n);
            write_out_box(ctx, file, f);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `soundfile(label, chan)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxSoundfile(
    b: *mut c_void,
    label: *mut *mut c_void,
    chan: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::Soundfile(l, c) = match_box(&ctx.arena, id) {
            write_out_box(ctx, label, l);
            write_out_box(ctx, chan, c);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Predicate for `cut` boxes.
///
/// # Safety
/// `t` must be a valid handle or null.
pub extern "C" fn CisBoxCut(t: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        matches!(match_box(&ctx.arena, id), BoxMatch::Cut)
    })
}

#[unsafe(no_mangle)]
/// Predicate for `environment` boxes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxEnvironment(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(match_box(&ctx.arena, id), BoxMatch::Environment)
    })
}

#[unsafe(no_mangle)]
/// Placeholder predicate for explicit error boxes (currently unsupported).
///
/// # Safety
/// `t` may be null or a valid handle.
pub extern "C" fn CisBoxError(_t: *mut c_void) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Matches `hbargraph(label, min, max)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxHBargraph(
    b: *mut c_void,
    lbl: *mut *mut c_void,
    min: *mut *mut c_void,
    max: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::HBargraph(l, mn, mx) = match_box(&ctx.arena, id) {
            write_out_box(ctx, lbl, l);
            write_out_box(ctx, min, mn);
            write_out_box(ctx, max, mx);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `hslider(label, cur, min, max, step)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxHSlider(
    b: *mut c_void,
    lbl: *mut *mut c_void,
    cur: *mut *mut c_void,
    min: *mut *mut c_void,
    max: *mut *mut c_void,
    step: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::HSlider(l, c, mn, mx, st) = match_box(&ctx.arena, id) {
            write_out_box(ctx, lbl, l);
            write_out_box(ctx, cur, c);
            write_out_box(ctx, min, mn);
            write_out_box(ctx, max, mx);
            write_out_box(ctx, step, st);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Match identifier boxes and return their symbol string.
///
/// # Safety
/// `str_out` must be null or a valid writable pointer.
pub unsafe extern "C" fn CisBoxIdent(t: *mut c_void, str_out: *mut *const c_char) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Ident(name) = match_box(&ctx.arena, id) {
            let owned = name.to_owned();
            if !str_out.is_null() {
                unsafe {
                    *str_out = ctx.intern_c_str_ptr(&owned);
                }
            }
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches integer literal boxes.
///
/// # Safety
/// `i` must be null or a valid writable pointer.
pub extern "C" fn CisBoxInt(t: *mut c_void, i: *mut c_int) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Int(v) = match_box(&ctx.arena, id) {
            write_out_int(i, v);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Placeholder matcher for metadata wrapper boxes (currently unsupported).
///
/// # Safety
/// All pointers may be null or valid writable pointers/handles.
pub extern "C" fn CisBoxMetadata(
    _b: *mut c_void,
    _exp: *mut *mut c_void,
    _mdlist: *mut *mut c_void,
) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Matches `num_entry(label, cur, min, max, step)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxNumEntry(
    b: *mut c_void,
    lbl: *mut *mut c_void,
    cur: *mut *mut c_void,
    min: *mut *mut c_void,
    max: *mut *mut c_void,
    step: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::NumEntry(l, c, mn, mx, st) = match_box(&ctx.arena, id) {
            write_out_box(ctx, lbl, l);
            write_out_box(ctx, cur, c);
            write_out_box(ctx, min, mn);
            write_out_box(ctx, max, mx);
            write_out_box(ctx, step, st);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Predicate for pattern-matcher root boxes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxPatternMatcher(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(match_box(&ctx.arena, id), BoxMatch::Case(_))
    })
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-0 box nodes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxPrim0(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(
            match_box(&ctx.arena, id),
            BoxMatch::Wire
                | BoxMatch::Cut
                | BoxMatch::Add
                | BoxMatch::Sub
                | BoxMatch::Mul
                | BoxMatch::Div
                | BoxMatch::Rem
                | BoxMatch::And
                | BoxMatch::Or
                | BoxMatch::Xor
                | BoxMatch::Lsh
                | BoxMatch::Rsh
                | BoxMatch::LRsh
                | BoxMatch::Lt
                | BoxMatch::Le
                | BoxMatch::Gt
                | BoxMatch::Ge
                | BoxMatch::Eq
                | BoxMatch::Ne
                | BoxMatch::Pow
                | BoxMatch::Acos
                | BoxMatch::Asin
                | BoxMatch::Atan
                | BoxMatch::Atan2
                | BoxMatch::Cos
                | BoxMatch::Sin
                | BoxMatch::Tan
                | BoxMatch::Exp
                | BoxMatch::Exp10
                | BoxMatch::Log
                | BoxMatch::Log10
                | BoxMatch::Sqrt
                | BoxMatch::Abs
                | BoxMatch::Fmod
                | BoxMatch::Remainder
                | BoxMatch::Floor
                | BoxMatch::Ceil
                | BoxMatch::Rint
                | BoxMatch::Round
                | BoxMatch::Delay
                | BoxMatch::Delay1
                | BoxMatch::Min
                | BoxMatch::Max
                | BoxMatch::Prefix
                | BoxMatch::IntCast
                | BoxMatch::FloatCast
                | BoxMatch::ReadOnlyTable
                | BoxMatch::WriteReadTable
                | BoxMatch::Select2
                | BoxMatch::Select3
                | BoxMatch::AssertBounds
                | BoxMatch::Lowest
                | BoxMatch::Highest
                | BoxMatch::Attach
                | BoxMatch::Enable
                | BoxMatch::Control
                | BoxMatch::Environment
        )
    })
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-1 box nodes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxPrim1(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(
            match_box(&ctx.arena, id),
            BoxMatch::Ident(_)
                | BoxMatch::Component(_)
                | BoxMatch::Library(_)
                | BoxMatch::Waveform(_)
                | BoxMatch::FFun(_)
                | BoxMatch::Case(_)
                | BoxMatch::PatternVar(_)
                | BoxMatch::Inputs(_)
                | BoxMatch::Outputs(_)
                | BoxMatch::Ondemand(_)
                | BoxMatch::Upsampling(_)
                | BoxMatch::Downsampling(_)
                | BoxMatch::Button(_)
                | BoxMatch::Checkbox(_)
        )
    })
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-2 box nodes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxPrim2(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(
            match_box(&ctx.arena, id),
            BoxMatch::Seq(_, _)
                | BoxMatch::Par(_, _)
                | BoxMatch::Rec(_, _)
                | BoxMatch::Split(_, _)
                | BoxMatch::Merge(_, _)
                | BoxMatch::Appl(_, _)
                | BoxMatch::Access(_, _)
                | BoxMatch::WithLocalDef(_, _)
                | BoxMatch::Abstr(_, _)
                | BoxMatch::Modulation(_, _)
                | BoxMatch::VGroup(_, _)
                | BoxMatch::HGroup(_, _)
                | BoxMatch::TGroup(_, _)
                | BoxMatch::Soundfile(_, _)
                | BoxMatch::VSlider(_, _, _, _, _)
                | BoxMatch::HSlider(_, _, _, _, _)
                | BoxMatch::NumEntry(_, _, _, _, _)
        )
    })
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-3 box nodes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxPrim3(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(
            match_box(&ctx.arena, id),
            BoxMatch::IPar(_, _, _)
                | BoxMatch::ISeq(_, _, _)
                | BoxMatch::ISum(_, _, _)
                | BoxMatch::IProd(_, _, _)
                | BoxMatch::WithRecDef(_, _, _)
                | BoxMatch::Route(_, _, _)
                | BoxMatch::Ffunction(_, _, _)
                | BoxMatch::FConst(_, _, _)
                | BoxMatch::FVar(_, _, _)
                | BoxMatch::VBargraph(_, _, _)
                | BoxMatch::HBargraph(_, _, _)
        )
    })
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-4 box nodes (currently none in this port).
///
/// # Safety
/// `b` may be null or a valid handle.
pub extern "C" fn CisBoxPrim4(_b: *mut c_void) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Predicate for primitive arity-5 box nodes (currently none in this port).
///
/// # Safety
/// `b` may be null or a valid handle.
pub extern "C" fn CisBoxPrim5(_b: *mut c_void) -> bool {
    false
}

#[unsafe(no_mangle)]
/// Matches floating-point literal boxes.
///
/// # Safety
/// `r` must be null or a valid writable pointer.
pub extern "C" fn CisBoxReal(t: *mut c_void, r: *mut f64) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let Some(v) = tree_to_double(&ctx.arena, id) {
            write_out_real(r, v);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches integer slot ids (best-effort mapping in this port).
///
/// # Safety
/// `id_out` must be null or a valid writable pointer.
pub extern "C" fn CisBoxSlot(t: *mut c_void, id_out: *mut c_int) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let Some(v) = tree_to_int(&ctx.arena, id).and_then(|v| c_int::try_from(v).ok()) {
            write_out_int(id_out, v);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches symbolic abstraction forms as `(slot, body)`.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxSymbolic(
    t: *mut c_void,
    slot: *mut *mut c_void,
    body: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        if let BoxMatch::Abstr(arg, expr) = match_box(&ctx.arena, id) {
            write_out_box(ctx, slot, arg);
            write_out_box(ctx, body, expr);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `vbargraph(label, min, max)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxVBargraph(
    b: *mut c_void,
    lbl: *mut *mut c_void,
    min: *mut *mut c_void,
    max: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::VBargraph(l, mn, mx) = match_box(&ctx.arena, id) {
            write_out_box(ctx, lbl, l);
            write_out_box(ctx, min, mn);
            write_out_box(ctx, max, mx);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Matches `vslider(label, cur, min, max, step)` boxes.
///
/// # Safety
/// All pointers must be null or valid writable pointers/handles.
pub extern "C" fn CisBoxVSlider(
    b: *mut c_void,
    lbl: *mut *mut c_void,
    cur: *mut *mut c_void,
    min: *mut *mut c_void,
    max: *mut *mut c_void,
    step: *mut *mut c_void,
) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        if let BoxMatch::VSlider(l, c, mn, mx, st) = match_box(&ctx.arena, id) {
            write_out_box(ctx, lbl, l);
            write_out_box(ctx, cur, c);
            write_out_box(ctx, min, mn);
            write_out_box(ctx, max, mx);
            write_out_box(ctx, step, st);
            true
        } else {
            false
        }
    })
}

#[unsafe(no_mangle)]
/// Predicate for waveform boxes.
///
/// # Safety
/// `b` must be a valid handle or null.
pub extern "C" fn CisBoxWaveform(b: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(b) else {
            return false;
        };
        matches!(match_box(&ctx.arena, id), BoxMatch::Waveform(_))
    })
}

#[unsafe(no_mangle)]
/// Predicate for wire boxes.
///
/// # Safety
/// `t` must be a valid handle or null.
pub extern "C" fn CisBoxWire(t: *mut c_void) -> bool {
    with_ctx(|ctx| {
        let Some(id) = ctx.decode(t) else {
            return false;
        };
        matches!(match_box(&ctx.arena, id), BoxMatch::Wire)
    })
}

#[unsafe(no_mangle)]
/// Compile Faust source into one flattened box.
///
/// # Safety
/// `error_msg` must be null or point to a writable buffer of at least 4096 bytes.
pub unsafe extern "C" fn CDSPToBoxes(
    name_app: *const c_char,
    dsp_content: *const c_char,
    _argc: c_int,
    _argv: *const *const c_char,
    inputs: *mut c_int,
    outputs: *mut c_int,
    error_msg: *mut c_char,
) -> *mut c_void {
    let source_name = match unsafe { utils::optional_c_str_arg(name_app, "name_app") } {
        Ok(Some(s)) if !s.is_empty() => s.to_owned(),
        Ok(_) => "FaustDSP".to_owned(),
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let content = match unsafe { utils::required_c_str_arg(dsp_content, "dsp_content") } {
        Ok(s) => s,
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let compiler = Compiler::new();
    let compiled = match compiler.compile_source_to_signals(&source_name, content) {
        Ok(v) => v,
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
            return std::ptr::null_mut();
        }
    };

    write_out_int(
        inputs,
        c_int::try_from(compiled.process_arity.inputs).unwrap_or_default(),
    );
    write_out_int(
        outputs,
        c_int::try_from(compiled.process_arity.outputs).unwrap_or_default(),
    );

    with_ctx(|ctx| {
        let mut memo = HashMap::new();
        match import_tree_rec(
            &compiled.parse.state.arena,
            &mut ctx.arena,
            compiled.process_box,
            &mut memo,
        ) {
            Some(id) => ctx.encode(id),
            None => {
                unsafe { utils::write_error_4096(error_msg, "failed to import process box tree") };
                std::ptr::null_mut()
            }
        }
    })
}

#[unsafe(no_mangle)]
/// Computes box input/output arity.
///
/// # Safety
/// `inputs`/`outputs` must be null or writable pointers.
pub extern "C" fn CgetBoxType(
    box_ptr: *mut c_void,
    inputs: *mut c_int,
    outputs: *mut c_int,
) -> bool {
    with_ctx(|ctx| {
        let Some(box_id) = ctx.decode(box_ptr) else {
            write_out_int(inputs, 0);
            write_out_int(outputs, 0);
            return false;
        };
        let flat = match try_build_flat_box(&ctx.arena, box_id) {
            Ok(flat) => flat,
            Err(_) => {
                write_out_int(inputs, 0);
                write_out_int(outputs, 0);
                return false;
            }
        };
        let mut cache = ArityCache::new();
        match box_arity_typed(&ctx.arena, flat, &mut cache) {
            Ok(arity) => {
                write_out_int(inputs, c_int::try_from(arity.inputs).unwrap_or_default());
                write_out_int(outputs, c_int::try_from(arity.outputs).unwrap_or_default());
                true
            }
            Err(_) => {
                write_out_int(inputs, 0);
                write_out_int(outputs, 0);
                false
            }
        }
    })
}

#[unsafe(no_mangle)]
/// Convert a box expression to a null-terminated signal array in normal form.
///
/// # Safety
/// `error_msg` must be null or point to a writable buffer of at least 4096 bytes.
pub unsafe extern "C" fn CboxesToSignals(
    box_ptr: *mut c_void,
    error_msg: *mut c_char,
) -> *mut *mut c_void {
    with_ctx(|ctx| {
        let Some(box_id) = ctx.decode(box_ptr) else {
            unsafe { utils::write_error_4096(error_msg, "null or unknown box pointer") };
            return std::ptr::null_mut();
        };
        let flat = match try_build_flat_box(&ctx.arena, box_id) {
            Ok(flat) => flat,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let mut cache = ArityCache::new();
        let arity = match box_arity_typed(&ctx.arena, flat, &mut cache) {
            Ok(a) => a,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let inputs = make_sig_input_list(&mut ctx.arena, arity.inputs);
        let outputs = match propagate_typed(&mut ctx.arena, flat, &inputs, &mut cache) {
            Ok(sigs) => sigs,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let raw: Vec<*mut c_void> = outputs.into_iter().map(|s| ctx.encode(s)).collect();
        ctx.alloc_handle_ptr_array(raw)
    })
}

#[unsafe(no_mangle)]
/// Convert a box expression to a null-terminated symbolic signal array.
///
/// # Safety
/// `error_msg` must be null or point to a writable buffer of at least 4096 bytes.
pub unsafe extern "C" fn CboxesToSignals2(
    box_ptr: *mut c_void,
    error_msg: *mut c_char,
) -> *mut *mut c_void {
    with_ctx(|ctx| {
        let Some(box_id) = ctx.decode(box_ptr) else {
            unsafe { utils::write_error_4096(error_msg, "null or unknown box pointer") };
            return std::ptr::null_mut();
        };
        let flat = match try_build_flat_box(&ctx.arena, box_id) {
            Ok(flat) => flat,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let mut cache = ArityCache::new();
        let arity = match box_arity_typed(&ctx.arena, flat, &mut cache) {
            Ok(a) => a,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let inputs = make_sig_input_list(&mut ctx.arena, arity.inputs);
        let outputs = match propagate_typed(&mut ctx.arena, flat, &inputs, &mut cache) {
            Ok(sigs) => sigs,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let mut symbolic = Vec::with_capacity(outputs.len());
        for signal in outputs {
            match de_bruijn_to_sym(&mut ctx.arena, signal) {
                Ok(sym) => symbolic.push(sym),
                Err(e) => {
                    unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                    return std::ptr::null_mut();
                }
            }
        }
        let raw: Vec<*mut c_void> = symbolic.into_iter().map(|s| ctx.encode(s)).collect();
        ctx.alloc_handle_ptr_array(raw)
    })
}

#[unsafe(no_mangle)]
/// Compile one box expression to target source code.
///
/// # Safety
/// `error_msg` must be null or point to a writable buffer of at least 4096 bytes.
pub unsafe extern "C" fn CcreateSourceFromBoxes(
    name_app: *const c_char,
    box_ptr: *mut c_void,
    lang: *const c_char,
    argc: c_int,
    argv: *const *const c_char,
    error_msg: *mut c_char,
) -> *mut c_char {
    let name_app = match unsafe { utils::optional_c_str_arg(name_app, "name_app") } {
        Ok(Some(s)) if !s.is_empty() => s.to_owned(),
        Ok(_) => "FaustDSP".to_owned(),
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let lang = match unsafe { utils::required_c_str_arg(lang, "lang") } {
        Ok(s) => s.to_ascii_lowercase(),
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let argv = match unsafe { utils::decode_c_argv(argc, argv) } {
        Ok(v) => v,
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };
    let parsed = match utils::parse_ffi_compile_args(&argv) {
        Ok(v) => v,
        Err(e) => {
            unsafe { utils::write_error_4096(error_msg, &e) };
            return std::ptr::null_mut();
        }
    };

    with_ctx(|ctx| {
        let Some(box_id) = ctx.decode(box_ptr) else {
            unsafe { utils::write_error_4096(error_msg, "null or unknown box pointer") };
            return std::ptr::null_mut();
        };

        let flat = match try_build_flat_box(&ctx.arena, box_id) {
            Ok(flat) => flat,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let mut cache = ArityCache::new();
        let arity = match box_arity_typed(&ctx.arena, flat, &mut cache) {
            Ok(a) => a,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };
        let inputs = make_sig_input_list(&mut ctx.arena, arity.inputs);
        let signals = match propagate_typed(&mut ctx.arena, flat, &inputs, &mut cache) {
            Ok(sigs) => sigs,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e.to_string()) };
                return std::ptr::null_mut();
            }
        };

        let module_name = parsed
            .module_name
            .clone()
            .unwrap_or_else(|| name_app.clone());
        let fir = match lower_signal_roots_to_fir(ctx, &signals, &module_name) {
            Ok(v) => v,
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e) };
                return std::ptr::null_mut();
            }
        };

        let rendered = render_fir_module_source(&fir, &lang, &module_name);

        match rendered {
            Ok(text) => utils::alloc_c_string(&text),
            Err(e) => {
                unsafe { utils::write_error_4096(error_msg, &e) };
                std::ptr::null_mut()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::sync::{Mutex, MutexGuard};

    static TEST_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

    fn fresh_test_context() -> MutexGuard<'static, ()> {
        let guard = TEST_CONTEXT_LOCK
            .lock()
            .expect("test context lock poisoned");
        createLibContext();
        guard
    }

    fn assert_box_matches(ptr: *mut c_void, expected: BoxMatch) {
        with_ctx(|ctx| {
            let id = ctx.decode(ptr).expect("test box handle must be known");
            assert_eq!(match_box(&ctx.arena, id), expected);
        });
    }

    unsafe fn printed_signal(ptr: *mut c_void) -> String {
        let raw = CprintSignal(ptr, false, 4096);
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("signal dump must be UTF-8")
            .to_owned();
        unsafe { freeCMemory(raw.cast()) };
        text
    }

    unsafe fn assert_signal_array_contract(
        convert: unsafe extern "C" fn(*mut c_void, *mut c_char) -> *mut *mut c_void,
    ) {
        let root = CboxWire();
        let mut err = vec![0_i8; 4096];
        let signals = unsafe { convert(root, err.as_mut_ptr()) };
        assert!(
            !signals.is_null(),
            "signal conversion failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );

        let first = unsafe { *signals };
        assert!(!first.is_null());
        assert!(unsafe { (*signals.add(1)).is_null() });
        let before_free = unsafe { printed_signal(first) };

        unsafe { freeCMemory(signals.cast()) };

        let after_free = unsafe { printed_signal(first) };
        assert_eq!(before_free, after_free);
    }

    unsafe fn create_source(root: *mut c_void, lang: &str) -> Result<String, String> {
        let name = CString::new("SourceSmoke").unwrap();
        let lang = CString::new(lang).unwrap();
        let mut err = vec![0_i8; 4096];
        let source = unsafe {
            CcreateSourceFromBoxes(
                name.as_ptr(),
                root,
                lang.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
            )
        };
        if source.is_null() {
            return Err(unsafe { CStr::from_ptr(err.as_ptr()) }
                .to_string_lossy()
                .into_owned());
        }
        let text = unsafe { CStr::from_ptr(source) }
            .to_str()
            .expect("source must be UTF-8")
            .to_owned();
        unsafe { freeCMemory(source.cast()) };
        Ok(text)
    }

    #[test]
    fn logical_and_arithmetic_right_shift_use_distinct_box_tags() {
        let _guard = fresh_test_context();

        let arsh = CboxARightShift();
        let lrsh = CboxLRightShift();

        assert_box_matches(arsh, BoxMatch::Rsh);
        assert_box_matches(lrsh, BoxMatch::LRsh);
        assert_ne!(arsh, lrsh);

        destroyLibContext();
    }

    #[test]
    fn exp10_uses_dedicated_box_tag() {
        let _guard = fresh_test_context();

        let exp = CboxExp();
        let exp10 = CboxExp10();

        assert_box_matches(exp, BoxMatch::Exp);
        assert_box_matches(exp10, BoxMatch::Exp10);
        assert_ne!(exp, exp10);

        destroyLibContext();
    }

    #[test]
    fn exp10_source_generation_keeps_exp10_math_call() {
        let _guard = fresh_test_context();

        let root = CboxExp10Aux(CboxWire());
        let name = CString::new("Exp10Smoke").unwrap();
        let lang = CString::new("fir").unwrap();
        let mut err = vec![0_i8; 4096];
        let source = unsafe {
            CcreateSourceFromBoxes(
                name.as_ptr(),
                root,
                lang.as_ptr(),
                0,
                std::ptr::null(),
                err.as_mut_ptr(),
            )
        };
        assert!(
            !source.is_null(),
            "source generation failed: {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        );
        let text = unsafe { CStr::from_ptr(source) }
            .to_str()
            .expect("FIR dump must be UTF-8")
            .to_owned();
        unsafe { freeCMemory(source.cast()) };

        assert!(text.contains("exp10"), "{text}");

        destroyLibContext();
    }

    #[test]
    fn boxes_to_signals_returns_null_terminated_context_owned_array() {
        let _guard = fresh_test_context();

        unsafe { assert_signal_array_contract(CboxesToSignals) };

        destroyLibContext();
    }

    #[test]
    fn boxes_to_signals2_returns_null_terminated_context_owned_array() {
        let _guard = fresh_test_context();

        unsafe { assert_signal_array_contract(CboxesToSignals2) };

        destroyLibContext();
    }

    #[test]
    fn create_source_from_boxes_supports_expected_backend_languages() {
        let _guard = fresh_test_context();

        for lang in ["c", "cpp", "fir", "interp"] {
            let source = unsafe { create_source(CboxWire(), lang) }
                .unwrap_or_else(|err| panic!("{lang} source generation failed: {err}"));
            assert!(!source.is_empty(), "{lang} source must not be empty");
        }

        destroyLibContext();
    }

    #[test]
    fn create_source_from_boxes_reports_unsupported_language() {
        let _guard = fresh_test_context();

        let err = unsafe { create_source(CboxWire(), "rust") }.expect_err("rust must be rejected");
        assert!(
            err.contains("unsupported lang 'rust' (expected c, cpp, fir, or interp)"),
            "{err}"
        );

        destroyLibContext();
    }
}
