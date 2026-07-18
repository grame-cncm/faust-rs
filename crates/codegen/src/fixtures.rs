//! Shared FIR fixtures for backend generation tests/examples.
//!
//! These builders define canonical FIR modules reused across multiple backend
//! tests (C, C++, and future backends) to avoid copy/paste drift.

use fir::{
    AccessType, BargraphType, ButtonType, FirBinOp, FirBuilder, FirId, FirMathOp, FirStore,
    FirType, NamedType, SliderRange, SliderType, UiBoxType,
};

/// Function pointer type for backend FIR fixture builders.
pub type FirFixtureBuilder = fn() -> (FirStore, FirId);

/// Named backend-oriented FIR fixtures available in this module.
///
/// The list is intentionally small and focused on backend bring-up scenarios:
/// simple audio loops, stateful DSP, control flow, tables, UI/meta, and
/// selected low-level FIR nodes.
#[must_use]
pub fn backend_test_fixtures() -> &'static [(&'static str, FirFixtureBuilder)] {
    &[
        ("sine_phasor", build_sine_phasor_test_module),
        ("heavy_bench", build_heavy_bench_test_module),
        ("passthrough", build_passthrough_test_module),
        ("gain_bias_ui_meta", build_gain_bias_ui_meta_test_module),
        ("table_state_delay", build_table_state_delay_test_module),
        ("control_flow", build_control_flow_test_module),
        ("math_intrinsics", build_math_intrinsics_test_module),
        ("ir_coverage", build_ir_coverage_test_module),
    ]
}

/// Returns the canonical `dsp` pointer type used by backend test modules.
fn obj_ptr_type() -> FirType {
    FirType::Ptr(Box::new(FirType::Obj))
}

/// Returns `FAUSTFLOAT*`.
fn faustfloat_ptr_type() -> FirType {
    FirType::Ptr(Box::new(FirType::FaustFloat))
}

/// Returns `FAUSTFLOAT**`.
fn faustfloat_ptr_ptr_type() -> FirType {
    FirType::Ptr(Box::new(FirType::Ptr(Box::new(FirType::FaustFloat))))
}

/// Canonical named-argument list for the DSP `compute` method.
fn compute_fun_args() -> [NamedType; 4] {
    [
        NamedType {
            name: "dsp".to_string(),
            typ: obj_ptr_type(),
        },
        NamedType {
            name: "count".to_string(),
            typ: FirType::Int32,
        },
        NamedType {
            name: "inputs".to_string(),
            typ: faustfloat_ptr_ptr_type(),
        },
        NamedType {
            name: "outputs".to_string(),
            typ: faustfloat_ptr_ptr_type(),
        },
    ]
}

/// Canonical FIR type for the DSP `compute` method.
fn compute_fun_type() -> FirType {
    FirType::Fun {
        args: vec![
            obj_ptr_type(),
            FirType::Int32,
            faustfloat_ptr_ptr_type(),
            faustfloat_ptr_ptr_type(),
        ],
        ret: Box::new(FirType::Void),
    }
}

/// Canonical named-argument list for `buildUserInterface`.
fn build_ui_fun_args() -> [NamedType; 2] {
    [
        NamedType {
            name: "dsp".to_string(),
            typ: obj_ptr_type(),
        },
        NamedType {
            name: "ui_interface".to_string(),
            typ: FirType::UI,
        },
    ]
}

/// Canonical FIR type for `buildUserInterface`.
fn build_ui_fun_type() -> FirType {
    FirType::Fun {
        args: vec![obj_ptr_type(), FirType::UI],
        ret: Box::new(FirType::Void),
    }
}

/// Canonical named-argument list for `metadata`.
fn metadata_fun_args() -> [NamedType; 2] {
    [
        NamedType {
            name: "dsp".to_string(),
            typ: obj_ptr_type(),
        },
        NamedType {
            name: "m".to_string(),
            typ: FirType::Meta,
        },
    ]
}

/// Canonical FIR type for `metadata`.
fn metadata_fun_type() -> FirType {
    FirType::Fun {
        args: vec![obj_ptr_type(), FirType::Meta],
        ret: Box::new(FirType::Void),
    }
}

/// Wraps globals/functions into a complete FIR `Module`.
fn module_with_functions(
    b: &mut FirBuilder<'_>,
    name: &str,
    globals: &[FirId],
    functions: &[FirId],
    num_inputs: usize,
    num_outputs: usize,
) -> FirId {
    let dsp_struct = b.block(&[]);
    let globals = b.block(globals);
    let functions = b.block(functions);
    let static_decls = b.block(&[]);
    b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
    )
}

/// Declares the canonical `compute` function around `body`.
fn declare_compute_fn(b: &mut FirBuilder<'_>, body: FirId) -> FirId {
    let args = compute_fun_args();
    b.declare_fun("compute", compute_fun_type(), &args, Some(body), false)
}

/// Creates `input0`/`output0` stack aliases for mono `compute` fixtures.
fn io_aliases_for_mono_compute(b: &mut FirBuilder<'_>) -> (FirId, FirId) {
    let chan0 = b.int32(0);
    let ptr_ty = faustfloat_ptr_type();
    let in_ptr = b.load_table("inputs", AccessType::FunArgs, chan0, ptr_ty.clone());
    let out_chan0 = b.int32(0);
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan0, ptr_ty.clone());
    let in_alias = b.declare_var("input0", ptr_ty.clone(), AccessType::Stack, Some(in_ptr));
    let out_alias = b.declare_var("output0", ptr_ty, AccessType::Stack, Some(out_ptr));
    (in_alias, out_alias)
}

/// Builds a canonical FIR module for a phasor-driven sine oscillator.
///
/// Module shape:
/// - UI controls: `freq` and `gain` sliders
/// - Stateful phase accumulator `fPhase`
/// - one output signal: `gain * sin(2*pi*phase)`
///
/// This fixture is intentionally backend-agnostic so all emitters can be
/// validated from the exact same FIR input.
///
/// Representative Faust DSP (approximate source intent):
/// ```faust
/// freq = hslider("freq", 440, 20, 3000, 1);
/// gain = hslider("gain", 0.2, 0, 1, 0.001);
/// phase = +(freq/48000.0) ~ _;
/// wrap(x) = x - float(x >= 1.0);
/// process = gain * sin(2.0*ma.PI * wrap(phase));
/// ```
#[must_use]
pub fn build_sine_phasor_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let f440 = b.float64(440.0);
    let f02 = b.float64(0.2);
    let f0 = b.float64(0.0);
    let dec_freq = b.declare_var("fFreq", FirType::FaustFloat, AccessType::Struct, Some(f440));
    let dec_gain = b.declare_var("fGain", FirType::FaustFloat, AccessType::Struct, Some(f02));
    let dec_phase = b.declare_var("fPhase", FirType::Float64, AccessType::Struct, Some(f0));
    let open = b.open_box(UiBoxType::Vertical, "Oscillator");
    let freq_slider = b.add_slider(
        SliderType::Horizontal,
        "freq",
        "fFreq",
        fir::SliderRange {
            init: 440.0,
            lo: 20.0,
            hi: 3000.0,
            step: 1.0,
        },
    );
    let gain_slider = b.add_slider(
        SliderType::Horizontal,
        "gain",
        "fGain",
        fir::SliderRange {
            init: 0.2,
            lo: 0.0,
            hi: 1.0,
            step: 0.001,
        },
    );
    let close = b.close_box();
    let build_ui_body = b.block(&[open, freq_slider, gain_slider, close]);
    let build_ui_args = build_ui_fun_args();
    let build_ui = b.declare_fun(
        "buildUserInterface",
        build_ui_fun_type(),
        &build_ui_args,
        Some(build_ui_body),
        false,
    );

    let freq = b.load_var("fFreq", AccessType::Struct, FirType::FaustFloat);
    let gain = b.load_var("fGain", AccessType::Struct, FirType::FaustFloat);
    let phase = b.load_var("fPhase", AccessType::Struct, FirType::Float64);
    let sample_rate = b.float64(48_000.0);
    let one = b.float64(1.0);
    let two_pi = b.float64(std::f64::consts::TAU);

    let phase_inc = b.binop(FirBinOp::Div, freq, sample_rate, FirType::Float64);
    let next_phase = b.binop(FirBinOp::Add, phase, phase_inc, FirType::Float64);
    let wrapped_phase_minus = b.binop(FirBinOp::Sub, next_phase, one, FirType::Float64);
    let wrap_cond = b.binop(FirBinOp::Ge, next_phase, one, FirType::Bool);
    let wrapped_phase = b.select2(wrap_cond, wrapped_phase_minus, next_phase, FirType::Float64);
    let store_phase = b.store_var("fPhase", AccessType::Struct, wrapped_phase);

    let phase_angle = b.binop(FirBinOp::Mul, two_pi, wrapped_phase, FirType::Float64);
    let sine = b.math_call(FirMathOp::Sin, &[phase_angle], FirType::Float64);
    let out = b.binop(FirBinOp::Mul, gain, sine, FirType::Float64);
    let out_chan = b.int32(0);
    let out_ptr_ty = FirType::Ptr(Box::new(FirType::FaustFloat));
    let out_ptr = b.load_table("outputs", AccessType::FunArgs, out_chan, out_ptr_ty.clone());
    let out_alias = b.declare_var("output0", out_ptr_ty, AccessType::Stack, Some(out_ptr));
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let store_out = b.store_table("output0", AccessType::Stack, i0, out);

    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let sample_body = b.block(&[store_phase, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, sample_body, false);
    let compute_body = b.block(&[out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);
    let module = module_with_functions(
        &mut b,
        "mydsp",
        &[dec_freq, dec_gain, dec_phase],
        &[build_ui, compute],
        0,
        1,
    );
    (store, module)
}

/// Builds a minimal mono passthrough FIR module (`output0[i] = input0[i]`).
///
/// Useful as the simplest backend smoke fixture for:
/// - function signature emission
/// - `inputs`/`outputs` pointer aliasing
/// - `SimpleForLoop`
/// - `LoadTable(kStack)` / `StoreTable(kStack)`
///
/// Representative Faust DSP:
/// ```faust
/// process = _;
/// ```
/// Builds a minimal mono passthrough FIR module (`output0[i] = input0[i]`).
///
/// This is the lowest-friction backend smoke fixture: simple I/O aliases and
/// no state, UI, or helper functions beyond `compute`.
#[must_use]
pub fn build_passthrough_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let sample = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, sample);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let module = module_with_functions(&mut b, "passthrough", &[], &[compute], 1, 1);
    (store, module)
}

/// Builds a mono gain/bias fixture with UI and metadata declarations.
///
/// Covers:
/// - `kStruct` globals
/// - UI ops (`OpenBox`, `AddSlider`, `AddButton`, `AddBargraph`, `CloseBox`)
/// - metadata declarations
/// - arithmetic + `Select2`
///
/// Representative Faust DSP (approximate source intent):
/// ```faust
/// declare name "gain-bias-ui-meta";
/// declare author "faust-rs";
///
/// gate = checkbox("gate");
/// gain = hslider("gain", 0.5, 0.0, 2.0, 0.001);
/// bias = nentry("bias", 0.0, -1.0, 1.0, 0.001);
///
/// level = hbargraph("level", 0.0, 1.0);
/// process(x) = ((x * gain) + bias) * gate;
/// ```
///
/// Note:
/// - The FIR fixture is a backend-oriented hand-written equivalent and may not
///   correspond 1:1 to a single normalized Faust source.
#[must_use]
pub fn build_gain_bias_ui_meta_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let f_gain_init = b.float64(0.5);
    let f_bias_init = b.float64(0.0);
    let f_gate_init = b.float64(1.0);
    let f_level_init = b.float64(0.0);
    let globals = [
        b.declare_var(
            "fGain",
            FirType::FaustFloat,
            AccessType::Struct,
            Some(f_gain_init),
        ),
        b.declare_var(
            "fBias",
            FirType::FaustFloat,
            AccessType::Struct,
            Some(f_bias_init),
        ),
        b.declare_var(
            "fGate",
            FirType::FaustFloat,
            AccessType::Struct,
            Some(f_gate_init),
        ),
        b.declare_var(
            "fLevel",
            FirType::FaustFloat,
            AccessType::Struct,
            Some(f_level_init),
        ),
    ];

    let open = b.open_box(UiBoxType::Vertical, "GainBias");
    let gate_btn = b.add_button(ButtonType::Checkbox, "gate", "fGate");
    let gain_slider = b.add_slider(
        SliderType::Horizontal,
        "gain",
        "fGain",
        SliderRange {
            init: 0.5,
            lo: 0.0,
            hi: 2.0,
            step: 0.001,
        },
    );
    let bias_slider = b.add_slider(
        SliderType::NumEntry,
        "bias",
        "fBias",
        SliderRange {
            init: 0.0,
            lo: -1.0,
            hi: 1.0,
            step: 0.001,
        },
    );
    let level_bg = b.add_bargraph(BargraphType::Horizontal, "level", "fLevel", 0.0, 1.0);
    let close = b.close_box();
    let build_ui_body = b.block(&[open, gate_btn, gain_slider, bias_slider, level_bg, close]);
    let build_ui_args = build_ui_fun_args();
    let build_ui = b.declare_fun(
        "buildUserInterface",
        build_ui_fun_type(),
        &build_ui_args,
        Some(build_ui_body),
        false,
    );

    let meta_name = b.add_meta_declare("0", "name", "gain-bias-ui-meta");
    let meta_author = b.add_meta_declare("0", "author", "faust-rs");
    let meta_body = b.block(&[meta_name, meta_author]);
    let meta_args = metadata_fun_args();
    let metadata = b.declare_fun(
        "metadata",
        metadata_fun_type(),
        &meta_args,
        Some(meta_body),
        false,
    );

    let reset_gain = b.store_var("fGain", AccessType::Struct, f_gain_init);
    let reset_bias = b.store_var("fBias", AccessType::Struct, f_bias_init);
    let reset_gate = b.store_var("fGate", AccessType::Struct, f_gate_init);
    let reset_ui_body = b.block(&[reset_gain, reset_bias, reset_gate]);
    let reset_ui = b.declare_fun(
        "instanceResetUserInterface",
        FirType::Fun {
            args: vec![obj_ptr_type()],
            ret: Box::new(FirType::Void),
        },
        &[NamedType {
            name: "dsp".to_string(),
            typ: obj_ptr_type(),
        }],
        Some(reset_ui_body),
        false,
    );

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let gain = b.load_var("fGain", AccessType::Struct, FirType::FaustFloat);
    let bias = b.load_var("fBias", AccessType::Struct, FirType::FaustFloat);
    let gate = b.load_var("fGate", AccessType::Struct, FirType::FaustFloat);
    let zero = b.float64(0.0);
    let gate_is_on = b.binop(FirBinOp::Gt, gate, zero, FirType::Bool);
    let gated = b.select2(gate_is_on, x, zero, FirType::FaustFloat);
    let scaled = b.binop(FirBinOp::Mul, gated, gain, FirType::FaustFloat);
    let y = b.binop(FirBinOp::Add, scaled, bias, FirType::FaustFloat);
    let y_f64 = b.cast(FirType::Float64, y);
    let level_abs = b.math_call(FirMathOp::Abs, &[y_f64], FirType::Float64);
    let store_level = b.store_var("fLevel", AccessType::Struct, level_abs);
    let store_out = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[store_level, store_out]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let module = module_with_functions(
        &mut b,
        "gain_bias_ui_meta",
        &globals,
        &[build_ui, metadata, reset_ui, compute],
        1,
        1,
    );
    (store, module)
}

/// Builds a stateful table-based mono fixture (write/read circular buffer).
///
/// Covers:
/// - `DeclareTable(kStruct)`
/// - `LoadTable/StoreTable(kStruct)`
/// - struct state updates (`fWriteIdx`)
/// - looped sample processing
///
/// Representative Faust DSP (approximate source intent):
/// ```faust
/// import("stdfaust.lib");
/// process = _ ~ @(4); // small fixed delay line behavior
/// ```
///
/// Note:
/// - The fixture uses an explicit FIR table and write index to expose backend
///   table/state lowering directly.
/// - The delay read is captured into an explicit temp-variable statement
///   (`tmpRead`) *before* the overwriting store to `fDelay`. FIR expressions
///   evaluate where they are textually embedded, not at their Rust-builder
///   construction point: an earlier version referenced the raw `load_table`
///   result directly from `write_out` (the *second* statement in the loop
///   body, after `write_delay`, the first), so the read re-evaluated against
///   the already-overwritten slot and the fixture was a silent passthrough
///   (`output == input`, no delay at all). See
///   `interp_delay_by_4_semantics_hold` below for the regression guard.
#[must_use]
pub fn build_table_state_delay_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let z = b.float32(0.0);
    let idx0 = b.int32(0);
    let globals = [
        b.declare_var("fWriteIdx", FirType::Int32, AccessType::Struct, Some(idx0)),
        b.declare_table(
            "fDelay",
            AccessType::Struct,
            FirType::FaustFloat,
            &[z, z, z, z],
        ),
    ];

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let idx = b.load_var("fWriteIdx", AccessType::Struct, FirType::Int32);
    let read = b.load_table("fDelay", AccessType::Struct, idx, FirType::FaustFloat);
    // Materialize the read before the overwrite below.
    let capture_read = b.declare_var(
        "tmpRead",
        FirType::FaustFloat,
        AccessType::Stack,
        Some(read),
    );
    let write_delay = b.store_table("fDelay", AccessType::Struct, idx, x);
    let read_captured = b.load_var("tmpRead", AccessType::Stack, FirType::FaustFloat);
    let write_out = b.store_table("output0", AccessType::Stack, i0, read_captured);
    let one_i = b.int32(1);
    let idx_plus = b.binop(FirBinOp::Add, idx, one_i, FirType::Int32);
    let four_i = b.int32(4);
    let ge_wrap = b.binop(FirBinOp::Ge, idx_plus, four_i, FirType::Bool);
    let zero_i = b.int32(0);
    let wrap = b.select2(ge_wrap, zero_i, idx_plus, FirType::Int32);
    let store_idx = b.store_var("fWriteIdx", AccessType::Struct, wrap);
    let loop_body = b.block(&[capture_read, write_delay, write_out, store_idx]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let module = module_with_functions(&mut b, "table_state_delay", &globals, &[compute], 1, 1);
    (store, module)
}

/// Builds a control-flow-heavy mono fixture for backend statement lowering.
///
/// Covers:
/// - `If`, `Switch`
/// - stack locals (`kStack`) and explicit sample-loop control flow
///
/// Representative Faust DSP (approximate source intent):
/// ```faust
/// // Sketch only: designed to exercise lowered FIR control-flow constructs.
/// mode = hslider("mode", 1, 0, 2, 1);
/// process(x) = select2(mode == 0, x,
///              select2(mode == 1, -x, abs(x)));
/// ```
///
/// Note:
/// - The explicit `switch` and conditional statements are intentionally
///   synthetic FIR stressors; they are not guaranteed to arise from a compact
///   Faust source exactly as written.
#[must_use]
pub fn build_control_flow_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let mode_init = b.int32(1);
    let globals = [b.declare_var("fMode", FirType::Int32, AccessType::Struct, Some(mode_init))];

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);

    let acc_zero = b.int32(0);
    let acc_decl = b.declare_var("acc", FirType::Int32, AccessType::Stack, Some(acc_zero));

    // Sample loop with switch/if/control producing the output.
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let neg_x = b.neg(x, FirType::FaustFloat);
    let zero_f = b.float64(0.0);
    let mode = b.load_var("fMode", AccessType::Struct, FirType::Int32);
    let store_x = b.store_table("output0", AccessType::Stack, i0, x);
    let mode_case0 = b.block(&[store_x]);
    let store_neg_x = b.store_table("output0", AccessType::Stack, i0, neg_x);
    let mode_case1 = b.block(&[store_neg_x]);
    let x_f64 = b.cast(FirType::Float64, x);
    let abs_x = b.math_call(FirMathOp::Abs, &[x_f64], FirType::Float64);
    let store_abs_x = b.store_table("output0", AccessType::Stack, i0, abs_x);
    let mode_default = b.block(&[store_abs_x]);
    let mode_switch = b.switch(
        mode,
        &[(0, mode_case0), (1, mode_case1)],
        Some(mode_default),
    );

    let x_is_pos = b.binop(FirBinOp::Gt, x, zero_f, FirType::Bool);
    let acc_cur = b.load_var("acc", AccessType::Stack, FirType::Int32);
    let acc_next = b.binop(FirBinOp::Add, acc_cur, mode, FirType::Int32);
    let store_acc = b.store_var("acc", AccessType::Stack, acc_next);
    let mode_one = b.int32(1);
    let mode_two = b.int32(2);
    let next_mode = b.select2(x_is_pos, mode_one, mode_two, FirType::Int32);
    let gated_stmt = b.store_var("fMode", AccessType::Struct, next_mode);
    let gated_then = b.block(&[gated_stmt]);
    let gated_if = b.if_(x_is_pos, gated_then, None);
    let drop_x = b.drop_(x);
    let then_block = b.block(&[drop_x]);
    let drop_neg_x = b.drop_(neg_x);
    let else_block = b.block(&[drop_neg_x]);
    let conditional_abs = b.if_(x_is_pos, then_block, Some(else_block));

    let sample_body = b.block(&[store_acc, gated_if, conditional_abs, mode_switch]);
    let sample_loop = b.simple_for_loop("i0", count, sample_body, false);

    let compute_body = b.block(&[in_alias, out_alias, acc_decl, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let module = module_with_functions(&mut b, "control_flow", &globals, &[compute], 1, 1);
    (store, module)
}

/// Builds a mono fixture focused on math intrinsics and numeric conversions.
///
/// Covers:
/// - unary/binary math calls (`sin`, `cos`, `pow`, `fmin`, `fmax`, `atan2`, ...)
/// - `Cast`, `Neg`, `BinOp`
///
/// Representative Faust DSP (approximate source intent):
/// ```faust
/// f(x) = (max(-1.0, min(pow(abs(x), 0.5), 1.0)) * 0.5) - atan2(sin(x), cos(x));
/// process = f;
/// ```
/// Builds a FIR module covering unary/binary math intrinsic lowering.
#[must_use]
pub fn build_math_intrinsics_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let xf64 = b.cast(FirType::Float64, x);
    let absx = b.math_call(FirMathOp::Abs, &[xf64], FirType::Float64);
    let sinx = b.math_call(FirMathOp::Sin, &[xf64], FirType::Float64);
    let cosx = b.math_call(FirMathOp::Cos, &[xf64], FirType::Float64);
    let half = b.float64(0.5);
    let powv = b.math_call(FirMathOp::Pow, &[absx, half], FirType::Float64);
    let one = b.float64(1.0);
    let minv = b.math_call(FirMathOp::Min, &[powv, one], FirType::Float64);
    let minus_one = b.float64(-1.0);
    let maxv = b.math_call(FirMathOp::Max, &[minv, minus_one], FirType::Float64);
    let atan = b.math_call(FirMathOp::Atan2, &[sinx, cosx], FirType::Float64);
    let half2 = b.float64(0.5);
    let scaled = b.binop(FirBinOp::Mul, maxv, half2, FirType::Float64);
    let neg_atan = b.neg(atan, FirType::Float64);
    let y = b.binop(FirBinOp::Add, scaled, neg_atan, FirType::Float64);
    let write = b.store_table("output0", AccessType::Stack, i0, y);
    let loop_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, loop_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let module = module_with_functions(&mut b, "math_intrinsics", &[], &[compute], 1, 1);
    (store, module)
}

/// Builds a large mono fixture intended for backend compute benchmarking.
///
/// Design goals:
/// - much heavier per-sample workload than `sine_phasor`
/// - deterministic and backend-agnostic FIR
/// - stress arithmetic + math-call lowering in a realistic stateful loop
///
/// Shape:
/// - one input / one output
/// - 12 persistent state variables (`s0..s11`)
/// - per-sample unrolled stage chain:
///   - mix accumulator with stage state
///   - apply stage scaling + cubic waveshaper
///   - store feedback into stage state
///   - accumulate feedback into running output
///
/// Representative Faust DSP intent (approximate):
/// ```faust
/// // Conceptual structure only, not exact normalized source.
/// process(x) = chainN(x) with {
///   stage(s, x) = y with {
///     z = (x + s) * c;
///     y = z - (z*z*z)*k;
///     s = y * 0.97;
///   };
/// };
/// ```
/// Builds a denser FIR module used for backend throughput/benchmark smoke checks.
#[must_use]
pub fn build_heavy_bench_test_module() -> (FirStore, FirId) {
    const STAGES: usize = 12;
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let mut globals = Vec::<FirId>::with_capacity(STAGES);
    let z = b.float32(0.0);
    for idx in 0..STAGES {
        let name = format!("s{idx}");
        globals.push(b.declare_var(&name, FirType::FaustFloat, AccessType::Struct, Some(z)));
    }

    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let acc_init = b.declare_var("acc", FirType::FaustFloat, AccessType::Stack, Some(x));

    let mut sample_stmts = Vec::<FirId>::with_capacity((STAGES * 2) + 2);
    sample_stmts.push(acc_init);
    for idx in 0..STAGES {
        let name = format!("s{idx}");
        let s = b.load_var(&name, AccessType::Struct, FirType::FaustFloat);
        let acc_cur = b.load_var("acc", AccessType::Stack, FirType::FaustFloat);
        let mixed = b.binop(FirBinOp::Add, acc_cur, s, FirType::FaustFloat);
        let coeff = b.float32((0.62 + 0.013 * idx as f64) as f32);
        let scaled = b.binop(FirBinOp::Mul, mixed, coeff, FirType::FaustFloat);
        let z2 = b.binop(FirBinOp::Mul, scaled, scaled, FirType::FaustFloat);
        let z3 = b.binop(FirBinOp::Mul, z2, scaled, FirType::FaustFloat);
        let curve_k = b.float32((0.05 + 0.004 * idx as f64) as f32);
        let shaped = b.binop(FirBinOp::Mul, z3, curve_k, FirType::FaustFloat);
        let nonlin = b.binop(FirBinOp::Sub, scaled, shaped, FirType::FaustFloat);
        let fb_gain = b.float32(0.97);
        let feedback = b.binop(FirBinOp::Mul, nonlin, fb_gain, FirType::FaustFloat);
        let store_state = b.store_var(&name, AccessType::Struct, feedback);
        sample_stmts.push(store_state);
        let acc_next = feedback;
        let store_acc = b.store_var("acc", AccessType::Stack, acc_next);
        sample_stmts.push(store_acc);
    }

    let acc_final = b.load_var("acc", AccessType::Stack, FirType::FaustFloat);
    let out_scale = b.float32((1.0 / STAGES as f64) as f32);
    let y = b.binop(FirBinOp::Mul, acc_final, out_scale, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, y);
    sample_stmts.push(write);

    let sample_body = b.block(&sample_stmts);
    let sample_loop = b.simple_for_loop("i0", count, sample_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);
    let module = module_with_functions(&mut b, "heavy_bench", &globals, &[compute], 1, 1);
    (store, module)
}

/// Builds a FIR module that intentionally exercises less common FIR nodes.
///
/// This fixture is primarily intended for backend/parser/debugger development,
/// not as a guaranteed runtime fixture for every backend.
///
/// Covered nodes include:
/// - `DeclareFun` prototype (`body=None`)
/// - `DeclareStructType`
/// - `DeclareBufferIterators`
/// - `Label`, `NullStatement`, `Drop`
/// - `LoadVarAddress`, `TeeVar`, `Bitcast`
/// - `IteratorForLoop`
///
/// Faust source provenance:
/// - No exact single Faust DSP source is expected for this fixture.
/// - This module is intentionally synthetic and hand-written in FIR to exercise
///   low-level nodes that are difficult or unstable to trigger from compact
///   Faust source programs.
///
/// Approximate runtime-facing Faust behavior of the `compute` entry:
/// ```faust
/// process = _; // runtime behavior is intentionally not the point of this fixture
/// ```
/// Builds a wide FIR module intended to touch many rarely used instruction families.
#[must_use]
pub fn build_ir_coverage_test_module() -> (FirStore, FirId) {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);

    let tmp_init = b.float64(0.0);
    let t0 = b.float32(0.0);
    let t1 = b.float32(1.0);
    let t2 = b.float32(2.0);
    let t3 = b.float32(3.0);
    let pow_proto_args = [
        NamedType {
            name: "x".into(),
            typ: FirType::Float64,
        },
        NamedType {
            name: "y".into(),
            typ: FirType::Float64,
        },
    ];
    let globals = [
        b.declare_var(
            "fTmp",
            FirType::FaustFloat,
            AccessType::Struct,
            Some(tmp_init),
        ),
        b.declare_table(
            "fTable",
            AccessType::Struct,
            FirType::Float32,
            &[t0, t1, t2, t3],
        ),
        b.declare_fun(
            "pow",
            FirType::Fun {
                args: vec![FirType::Float64, FirType::Float64],
                ret: Box::new(FirType::Float64),
            },
            &pow_proto_args,
            None,
            false,
        ),
    ];

    let label = b.label("coverage-start");
    let null_stmt = b.null_statement();
    let buf_iters = b.declare_buffer_iterators("it0", "it1", 2, FirType::FaustFloat, true, false);
    let tmp_one = b.int32(1);
    let tmp_decl = b.declare_var("tmp", FirType::Int32, AccessType::Stack, Some(tmp_one));
    let ftmp_addr = b.load_var_address(
        "fTmp",
        AccessType::Struct,
        FirType::Ptr(Box::new(FirType::FaustFloat)),
    );
    let drop_addr = b.drop_(ftmp_addr);
    let tmp_load = b.load_var("tmp", AccessType::Stack, FirType::Int32);
    let one_i = b.int32(1);
    let tmp_inc = b.binop(FirBinOp::Add, tmp_load, one_i, FirType::Int32);
    let tee_tmp = b.tee_var("tmp", AccessType::Stack, tmp_inc, FirType::Int32);
    let drop_tee = b.drop_(tee_tmp);
    let one_f64 = b.float64(1.0);
    let one_f32 = b.cast(FirType::Float32, one_f64);
    let bitcast_i32 = b.bitcast(FirType::Int32, one_f32);
    let drop_bitcast = b.drop_(bitcast_i32);
    let iter_body_stmt = b.null_statement();
    let iter_body = b.block(&[iter_body_stmt]);
    let iter_loop = b.iterator_for_loop(&["it0", "it1"], false, iter_body);
    let ret_void = b.ret(None);
    let helper_body = b.block(&[
        label,
        null_stmt,
        buf_iters,
        tmp_decl,
        drop_addr,
        drop_tee,
        drop_bitcast,
        iter_loop,
        ret_void,
    ]);
    let helper = b.declare_fun(
        "coverageHelper",
        FirType::Fun {
            args: vec![],
            ret: Box::new(FirType::Void),
        },
        &[],
        Some(helper_body),
        false,
    );

    // Keep a valid simple compute entry for backend smoke paths.
    let (in_alias, out_alias) = io_aliases_for_mono_compute(&mut b);
    let count = b.load_var("count", AccessType::FunArgs, FirType::Int32);
    let i0 = b.load_var("i0", AccessType::Loop, FirType::Int32);
    let x = b.load_table("input0", AccessType::Stack, i0, FirType::FaustFloat);
    let ftmp = b.load_var("fTmp", AccessType::Struct, FirType::FaustFloat);
    let y = b.binop(FirBinOp::Add, x, ftmp, FirType::FaustFloat);
    let write = b.store_table("output0", AccessType::Stack, i0, y);
    let sample_body = b.block(&[write]);
    let sample_loop = b.simple_for_loop("i0", count, sample_body, false);
    let compute_body = b.block(&[in_alias, out_alias, sample_loop]);
    let compute = declare_compute_fn(&mut b, compute_body);

    let struct_decl = b.declare_struct_type(FirType::Struct(
        "coverage_dsp".into(),
        vec![FirType::FaustFloat],
    ));
    let dsp_struct = b.block(&[struct_decl]);
    let globals_block = b.block(&globals);
    let functions = b.block(&[helper, compute]);
    let static_decls = b.block(&[]);
    let module = b.module(
        1,
        1,
        "ir_coverage",
        dsp_struct,
        globals_block,
        functions,
        static_decls,
    );
    (store, module)
}

#[cfg(test)]
mod tests {
    use fir::{FirMatch, match_fir};

    use super::{
        backend_test_fixtures, build_sine_phasor_test_module, build_table_state_delay_test_module,
    };
    use crate::backends::interp::{FbcDspInstance, InterpOptions, generate_interp_module};

    #[test]
    fn sine_fixture_is_still_exposed() {
        let (store, module) = build_sine_phasor_test_module();
        assert!(matches!(match_fir(&store, module), FirMatch::Module { .. }));
    }

    #[test]
    fn all_backend_fixtures_build_module_roots() {
        for (name, build) in backend_test_fixtures() {
            let (store, module) = build();
            match match_fir(&store, module) {
                FirMatch::Module { .. } => {}
                other => panic!("fixture {name} did not produce a module root: {other:?}"),
            }
        }
    }

    /// Regression guard for the read-before-write ordering bug: executes
    /// `table_state_delay` through the interpreter and asserts genuine
    /// delay-by-4 semantics (`output[i] == 0` for `i < 4`, `output[i] ==
    /// input[i - 4]` for `i >= 4`), not the passthrough (`output ==
    /// input`) an earlier, incorrectly ordered version of this fixture
    /// silently produced.
    #[test]
    fn interp_delay_by_4_semantics_hold() {
        let (store, module) = build_table_state_delay_test_module();
        let options = InterpOptions {
            opt_level: 0,
            module_name: None,
        };
        let mut factory = generate_interp_module::<f32>(&store, module, &options)
            .expect("interp codegen should succeed");
        let mut instance = FbcDspInstance::new(&mut factory);
        instance.init(44_100);

        let input: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        let mut output = vec![0.0f32; 10];
        {
            let in_refs: [&[f32]; 1] = [&input[..]];
            let mut out_refs: [&mut [f32]; 1] = [&mut output[..]];
            instance
                .try_compute(10, &in_refs, &mut out_refs)
                .expect("compute should succeed");
        }

        let expected = [0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        assert_eq!(
            output, expected,
            "table_state_delay must produce genuine delay-by-4 output, not a passthrough"
        );
    }
}
