//! PV — early vertical vector execution slice: end-to-end bit-exactness.
//!
//! Plan references: `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`,
//! section "PV - Early vertical vector execution slice"; certified plan,
//! section "RV - Early executable vertical slice".
//!
//! This test drives the DSP built by
//! `transform::signal_fir::pv_slice::build_pv_signals` through three
//! independently emitted FIR modules — the existing scalar fast-lane
//! pipeline (reference), and the two new vector loop variants
//! (`PvLoopVariant::Lv0`/`Lv1`) — executed through the same interpreter
//! backend across a sequence of blocks (including a non-dividing tail
//! block), and asserts bit-for-bit output agreement plus a topology
//! assertion that would fail if the vector path silently collapsed to one
//! loop or serialized everything.
//!
//! Not wired into any production compile path — this is additive evidence,
//! per the PV pass criterion ("one nontrivial `VectorPlan`-to-backend slice
//! is bit-exact for `-lv 0` and `-lv 1`"), not a new compiler feature.

use codegen::backends::interp::{FbcDspInstance, InterpOptions, generate_interp_module};
use fir::{FirId, FirStore};
use transform::signal_fir::pv_slice::{
    PvLoopId, PvLoopVariant, build_pv_plan, build_pv_signals, pv_schedule, route_pv_vector_fir,
};
use transform::signal_fir::{RealType, SignalFirOptions, compile_signals_to_fir_fastlane_with_ui};
use transform::signal_prepare::prepare_signals_for_fir;
use ui::UiProgram;

/// Strictly larger than every block size this test uses, so no delay read
/// ever needs a same-block transported value (see `pv_slice` module docs).
const DELAY_AMOUNT: i32 = 20;
/// Covers every block size below with headroom.
const MAX_BLOCK: usize = 16;
/// A tail block deliberately not a multiple of `pv_slice::LV0_CHUNK` (4),
/// exercising the `Lv0` remainder loop, plus a total run length well past
/// `DELAY_AMOUNT` so the delayed output is observed for many samples.
const BLOCK_SIZES: &[usize] = &[7, 8, 8, 5, 3, 11, 6];

fn run_module(store: &FirStore, module: FirId, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let options = InterpOptions {
        opt_level: 0,
        module_name: None,
    };
    let mut factory =
        generate_interp_module::<f32>(store, module, &options).expect("interp codegen");
    let mut instance = FbcDspInstance::new(&mut factory);
    instance.init(44_100);

    let mut y_out = Vec::with_capacity(input.len());
    let mut z_out = Vec::with_capacity(input.len());
    let mut pos = 0usize;
    for &n in BLOCK_SIZES {
        if pos >= input.len() {
            break;
        }
        let n = n.min(input.len() - pos);
        let in_block = &input[pos..pos + n];
        let mut out0 = vec![0.0f32; n];
        let mut out1 = vec![0.0f32; n];
        {
            let in_refs: [&[f32]; 1] = [in_block];
            let mut out_refs: [&mut [f32]; 2] = [&mut out0, &mut out1];
            instance
                .try_compute(n as i32, &in_refs, &mut out_refs)
                .unwrap_or_else(|e| panic!("compute failed at pos={pos} n={n}: {e:?}"));
        }
        y_out.extend_from_slice(&out0);
        z_out.extend_from_slice(&out1);
        pos += n;
    }
    (y_out, z_out)
}

/// Deterministic, non-trivial input: not a pure ramp (which could mask
/// arithmetic-ordering bugs under linearity), long enough to exceed
/// `DELAY_AMOUNT` several times over.
fn deterministic_input(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32;
            (t * 0.31).sin() + 0.05 * t - if i % 7 == 0 { 0.2 } else { 0.0 }
        })
        .collect()
}

#[test]
fn pv_vector_slice_topology_is_genuinely_split() {
    let (arena, y, z) = build_pv_signals(DELAY_AMOUNT);
    let plan = build_pv_plan(&arena, y, z, MAX_BLOCK);

    // Snapshot: loop roots, the cross-loop edge, transport type/length.
    let order = pv_schedule(&plan);
    assert_eq!(
        order,
        vec![PvLoopId::OwnsX, PvLoopId::ConsumesTransport],
        "topology assertion: exactly two loops in dependency order"
    );
    assert_eq!(plan.transport.signal, plan.x);
    assert!(matches!(plan.transport.elem_type, fir::FirType::FaustFloat));
    assert_eq!(plan.transport.max_length, MAX_BLOCK);
}

#[test]
fn pv_vector_slice_is_bit_exact_for_both_loop_variants() {
    let total: usize = BLOCK_SIZES.iter().sum();
    let input = deterministic_input(total);

    // Scalar reference: the existing, unrelated fast-lane pipeline.
    let (arena, y, z) = build_pv_signals(DELAY_AMOUNT);
    let _prepared = prepare_signals_for_fir(&arena, &[y, z], &UiProgram::empty())
        .expect("PV signal forest should prepare");
    let scalar_fir = compile_signals_to_fir_fastlane_with_ui(
        &arena,
        &[y, z],
        1,
        2,
        &UiProgram::empty(),
        &SignalFirOptions {
            module_name: "pv_scalar".to_owned(),
            real_type: RealType::Float32,
            ..SignalFirOptions::default()
        },
    )
    .expect("scalar fast-lane lowering should succeed for the PV DSP");
    let (scalar_y, scalar_z) = run_module(&scalar_fir.store, scalar_fir.module, &input);

    // New signal-level vector path: the plan is built once and routed twice.
    let plan = build_pv_plan(&arena, y, z, MAX_BLOCK);
    let _ = pv_schedule(&plan); // -ss 0, independently checked (see pv_slice tests)

    let (store_lv0, module_lv0) = route_pv_vector_fir(&plan, PvLoopVariant::Lv0);
    let (lv0_y, lv0_z) = run_module(&store_lv0, module_lv0, &input);

    let (store_lv1, module_lv1) = route_pv_vector_fir(&plan, PvLoopVariant::Lv1);
    let (lv1_y, lv1_z) = run_module(&store_lv1, module_lv1, &input);

    assert_eq!(scalar_y.len(), total);
    assert_eq!(lv0_y.len(), total);
    assert_eq!(lv1_y.len(), total);

    assert_eq!(
        lv0_y, scalar_y,
        "-lv 0 output y must be bit-exact vs scalar"
    );
    assert_eq!(
        lv0_z, scalar_z,
        "-lv 0 output z must be bit-exact vs scalar"
    );
    assert_eq!(
        lv1_y, scalar_y,
        "-lv 1 output y must be bit-exact vs scalar"
    );
    assert_eq!(
        lv1_z, scalar_z,
        "-lv 1 output z must be bit-exact vs scalar"
    );

    // The delayed output must be non-trivial (not silently all-zero): after
    // DELAY_AMOUNT samples it should track the (scaled) input history.
    let nonzero_after_delay = scalar_y
        .iter()
        .skip(DELAY_AMOUNT as usize)
        .any(|v| v.abs() > 1e-6);
    assert!(
        nonzero_after_delay,
        "sanity: the delayed output must carry real signal, not just zeros"
    );
}
